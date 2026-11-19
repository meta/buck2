/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

'use client';

import {
  createContext,
  useContext,
  useState,
  useEffect,
  useCallback,
  useRef,
  type ReactNode,
} from 'react';
import type {EventSummary} from '../../lib/event-log-decoder';
import type {StreamingWorkerResponse} from '../../lib/streaming-decode-worker';
import {useBackend} from '../../backend';
import type {AggregateData} from '../../lib/streaming-collectors';
import type {CriticalPathData} from '../../lib/critical-path';
import type {PhaseTimings, TimelineLane} from '../../lib/phase-timer';
import {PhaseTimer, SpanRecorder, nowMs} from '../../lib/phase-timer';
import {ParallelDecoder} from '../../lib/parallel-decode/orchestrator';
import {
  type LazyAggregateName,
  readLazyAggregate,
  writeLazyAggregate,
} from '../../lib/event-log-cache';
import {ChunkLRUCache} from '../../lib/chunk-cache';
import {EventSummaryStore} from '../../lib/event-summary-store';
import {
  getCachedInvocation,
  getCachedChunk,
  loadCachedSummaryBatches,
  deleteCachedInvocation,
} from '../../lib/event-log-cache';

export type EventLogState =
  | {status: 'idle'}
  | {
      status: 'loading';
      progress: string;
      decompressedBytes?: number;
      phaseTimings?: PhaseTimings;
      /** Number of events ingested into the store so far. */
      eventsLoaded?: number;
      /** `Date.now()` snapshot taken when fetchAndDecode started; consumers
       *  derive elapsed time as `Date.now() - loadStartedMs`. */
      loadStartedMs: number;
    }
  | {status: 'error'; message: string}
  | {
      status: 'loaded';
      rawSize: number;
      decompressedSize: number;
      /** Total events in the store. */
      totalEvents: number;
      /** End-to-end fetch + decode wall time, in milliseconds. */
      fetchDecodeMs: number;
      /** Final phase-time breakdown from the worker. */
      phaseTimings?: PhaseTimings;
      /** Per-worker swim-lane timeline. Only populated for the parallel
       *  pipeline; undefined for single-worker / cached loads. */
      timelineLanes?: TimelineLane[];
      /**
       * Columnar storage for event summaries. Use direct accessors
       * (`store.getType(i)`, `store.getEventType(i)`, etc.) for performance,
       * or `store.get(i)` to materialize a full EventSummary object.
       */
      summaries: EventSummaryStore;
      getEventData: (summary: EventSummary) => Record<string, unknown>;
      /**
       * Like `getEventData`, but for large logs awaits any required chunk
       * load instead of returning empty on cache miss. Use this when you
       * need a guaranteed result (e.g. one-shot lookups for buildGraphInfo);
       * use the sync version for hot paths that tolerate empty results.
       */
      getEventDataAsync: (
        summary: EventSummary,
      ) => Promise<Record<string, unknown>>;
      getEventBytes: (summary: EventSummary) => Uint8Array;
      /** Small inline aggregates available immediately. The big lists
       *  (loadSpans/analysisSpans/actionSpans) load lazily — see
       *  `getLazyAggregate` and the `useLazyAggregate` hook. */
      aggregates: {criticalPath: CriticalPathData | null};
      /** Fetch a lazy aggregate. Caches the in-flight Promise and the
       *  resolved value so repeat calls within an invocation share one
       *  IDB read. Returns the typed array based on `name`. */
      getLazyAggregate: <K extends LazyAggregateName>(
        name: K,
      ) => Promise<LazyAggregateValue<K>>;
      isLargeLog: boolean;
    };

/** Type-level mapping for lazy aggregates. */
export interface LazyAggregateValueMap {
  loadSpans: AggregateData['loadSpans'];
  analysisSpans: AggregateData['analysisSpans'];
  actionSpans: AggregateData['actionSpans'];
}
export type LazyAggregateValue<K extends LazyAggregateName> =
  LazyAggregateValueMap[K];

const EventLogContext = createContext<EventLogState>({status: 'idle'});

export function useEventLog(): EventLogState {
  return useContext(EventLogContext);
}

export interface LazyAggregateResult<T> {
  data: T | null;
  loading: boolean;
  error: string | null;
}

/**
 * Hook for consuming a lazy aggregate (loadSpans / analysisSpans /
 * actionSpans). Returns `{data, loading, error}`. Resolves immediately
 * if the aggregate was already fetched (or prefilled by a fresh decode);
 * otherwise triggers a one-shot IDB read shared across all consumers.
 *
 * Returns all-nulls + loading=false when the event log isn't loaded yet.
 */
export function useLazyAggregate<K extends LazyAggregateName>(
  name: K,
): LazyAggregateResult<LazyAggregateValueMap[K]> {
  const logState = useEventLog();
  const [result, setResult] = useState<
    LazyAggregateResult<LazyAggregateValueMap[K]>
  >({
    data: null,
    loading: false,
    error: null,
  });

  useEffect(() => {
    if (logState.status !== 'loaded') {
      setResult({data: null, loading: false, error: null});
      return;
    }
    let cancelled = false;
    setResult({data: null, loading: true, error: null});
    logState
      .getLazyAggregate(name)
      .then(data => {
        if (cancelled) return;
        setResult({data, loading: false, error: null});
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setResult({
          data: null,
          loading: false,
          error: err instanceof Error ? err.message : String(err),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [logState, name]);

  return result;
}

const EMPTY_AGGREGATES: AggregateData = {
  loadSpans: [],
  actionSpans: [],
  analysisSpans: [],
  criticalPath: null,
};

/** Per-invocation cache of lazy aggregates. Cleared whenever the active
 *  eventLogPath changes. Holds the in-flight Promise so concurrent
 *  consumers share one IDB read. */
type LazyAggregateCache = {
  [K in LazyAggregateName]?: Promise<LazyAggregateValueMap[K]>;
};

function emptyLazyAggregate<K extends LazyAggregateName>(
  _name: K,
): LazyAggregateValueMap[K] {
  // All three lazy aggregates are arrays. Returning an empty array of the
  // appropriate union type keeps the type signature uniform.
  return [] as unknown as LazyAggregateValueMap[K];
}

/**
 * Reject if the given Promise hasn't settled within `ms` milliseconds.
 * Used to defend against IndexedDB transactions that get queued behind a
 * stuck write — we'd rather fall through to a fresh download than hang the
 * whole UI on the cache lookup.
 */
function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(() => reject(new Error(label)), ms);
    p.then(
      v => {
        clearTimeout(t);
        resolve(v);
      },
      e => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}

export function EventLogProvider({
  eventLogPath,
  children,
}: {
  eventLogPath: string | null;
  children: ReactNode;
}) {
  const [state, setState] = useState<EventLogState>({status: 'idle'});
  const backend = useBackend();

  // Small-log: uncompressed proto ArrayBuffers (in memory)
  const batchBuffersRef = useRef<ArrayBuffer[]>([]);
  // Large-log: chunks are in IndexedDB, accessed on demand
  const chunkCacheRef = useRef(new ChunkLRUCache());
  const modeRef = useRef<'small' | 'large'>('small');
  const aggregatesRef = useRef<AggregateData>(EMPTY_AGGREGATES);
  const eventLogPathRef = useRef<string | null>(null);
  // Per-invocation cache of lazy aggregate Promises. Reset whenever the
  // active eventLogPath changes (in the useEffect cleanup).
  const lazyAggregateCacheRef = useRef<LazyAggregateCache>({});

  // Lazy-load the proto decoder
  const decoderRef = useRef<
    typeof import('../../lib/event-log-decoder') | null
  >(null);

  const getDecoder = useCallback(async () => {
    if (!decoderRef.current) {
      decoderRef.current = await import('../../lib/event-log-decoder');
    }
    return decoderRef.current;
  }, []);

  // Read a chunk: from memory (small log) or IndexedDB + LRU cache (large log)
  const getChunkData = useCallback(
    async (chunkIndex: number): Promise<Uint8Array> => {
      if (modeRef.current === 'small') {
        const buffer = batchBuffersRef.current[chunkIndex];
        return buffer ? new Uint8Array(buffer) : new Uint8Array(0);
      }

      // Large log: check LRU cache first
      const cached = chunkCacheRef.current.get(chunkIndex);
      if (cached) return cached;

      // Read from IndexedDB
      const path = eventLogPathRef.current;
      if (!path) return new Uint8Array(0);

      const buffer = await getCachedChunk(path, chunkIndex);
      if (!buffer) return new Uint8Array(0);

      const data = new Uint8Array(buffer);
      chunkCacheRef.current.put(chunkIndex, data);
      return data;
    },
    [],
  );

  // Synchronous chunk access for getEventData/getEventBytes
  // Uses the LRU cache — if the chunk isn't cached, returns empty
  // (caller should ensure chunk is loaded first for large logs)
  const getChunkSync = useCallback((chunkIndex: number): Uint8Array => {
    if (modeRef.current === 'small') {
      const buffer = batchBuffersRef.current[chunkIndex];
      return buffer ? new Uint8Array(buffer) : new Uint8Array(0);
    }
    return chunkCacheRef.current.get(chunkIndex) ?? new Uint8Array(0);
  }, []);

  const getEventData = useCallback(
    (summary: EventSummary): Record<string, unknown> => {
      const decoder = decoderRef.current;
      if (!decoder) return {};

      const chunk = getChunkSync(summary.batchIndex);
      if (chunk.length === 0) {
        // For large logs, the chunk might not be in the LRU cache yet.
        // Trigger an async load and return empty for now.
        // The caller should re-render when the chunk is available.
        if (modeRef.current === 'large') {
          getChunkData(summary.batchIndex); // fire-and-forget async load
        }
        return {};
      }

      const bytes = chunk.subarray(
        summary.offsetInBatch,
        summary.offsetInBatch + summary.lengthInBatch,
      );
      return decoder.decodeEventFromProto(bytes, summary.type === 'invocation');
    },
    [getChunkSync, getChunkData],
  );

  const getEventBytes = useCallback(
    (summary: EventSummary): Uint8Array => {
      const chunk = getChunkSync(summary.batchIndex);
      if (chunk.length === 0) return new Uint8Array(0);
      return chunk.subarray(
        summary.offsetInBatch,
        summary.offsetInBatch + summary.lengthInBatch,
      );
    },
    [getChunkSync],
  );

  const getEventDataAsync = useCallback(
    async (summary: EventSummary): Promise<Record<string, unknown>> => {
      const decoder = decoderRef.current;
      if (!decoder) return {};

      // Ensure the chunk is loaded. For small logs this is a fast in-memory
      // lookup; for large logs this awaits the IDB read + LRU population.
      const chunk = await getChunkData(summary.batchIndex);
      if (chunk.length === 0) return {};

      const bytes = chunk.subarray(
        summary.offsetInBatch,
        summary.offsetInBatch + summary.lengthInBatch,
      );
      return decoder.decodeEventFromProto(bytes, summary.type === 'invocation');
    },
    [getChunkData],
  );

  /**
   * Fetch a lazy aggregate (loadSpans / analysisSpans / actionSpans),
   * caching the in-flight Promise per (eventLogPath, name) so concurrent
   * consumers share one IDB read.
   *
   * If the aggregate was set via `prefillLazyAggregate` (used after a
   * fresh-decode `done` arrives so we don't have to round-trip through
   * IDB), the cached Promise resolves immediately.
   */
  const getLazyAggregate = useCallback(
    <K extends LazyAggregateName>(
      name: K,
    ): Promise<LazyAggregateValueMap[K]> => {
      const cache = lazyAggregateCacheRef.current as Record<
        LazyAggregateName,
        Promise<unknown> | undefined
      >;
      const existing = cache[name] as
        Promise<LazyAggregateValueMap[K]> | undefined;
      if (existing) return existing;
      const path = eventLogPathRef.current;
      if (!path) return Promise.resolve(emptyLazyAggregate(name));
      const fetchPromise = readLazyAggregate<LazyAggregateValueMap[K]>(
        path,
        name,
      ).then(v => v ?? emptyLazyAggregate(name));
      cache[name] = fetchPromise;
      return fetchPromise;
    },
    [],
  );

  useEffect(() => {
    if (!eventLogPath) {
      setState({status: 'error', message: 'Event log path not available'});
      return;
    }

    let cancelled = false;
    batchBuffersRef.current = [];
    chunkCacheRef.current.clear();
    lazyAggregateCacheRef.current = {};
    eventLogPathRef.current = eventLogPath;

    async function fetchAndDecode() {
      // URL params: ?clearLogCache=1 deletes any cached entry and forces a
      // fresh download. ?skipLogCache=1 skips reading the cache (still writes).
      const params =
        typeof window !== 'undefined'
          ? new URLSearchParams(window.location.search)
          : new URLSearchParams();
      const clearCache = params.get('clearLogCache') === '1';
      const skipCache = clearCache || params.get('skipLogCache') === '1';

      if (clearCache) {
        try {
          await deleteCachedInvocation(eventLogPath!);
        } catch {
          // ignore
        }
      }

      const loadStartedMs = Date.now();
      // Tracks main-thread phases — currently only the cache-hit path
      // populates this, since the fresh-download path runs all timed work
      // inside the worker(s).
      const mainTimer = new PhaseTimer();
      // Coarse main-thread spans for the timeline visualization. Recorded
      // for the parallel pipeline; assembled into a 'main' TimelineLane
      // and prepended before publishing.
      const mainSpans = new SpanRecorder();
      setState({
        status: 'loading',
        progress: 'Loading proto decoder...',
        loadStartedMs,
      });
      await getDecoder();

      if (cancelled) return;
      setState({
        status: 'loading',
        progress: 'Reading cache entry...',
        loadStartedMs,
      });

      // Check IndexedDB cache first (unless explicitly skipped). Wrap with a
      // 10s timeout — if IndexedDB is locked (a previous decoder still
      // holding writes, a stuck schema upgrade, etc.) we'd otherwise hang
      // here forever. Fall through to a fresh download instead.
      try {
        const cached = skipCache
          ? null
          : await mainTimer.timeAsync('cache_read_invocation', () =>
              withTimeout(
                getCachedInvocation(eventLogPath!, mainTimer),
                10_000,
                'cache lookup timed out',
              ),
            );
        if (cached && !cancelled) {
          setState({
            status: 'loading',
            progress: `Loading ${cached.totalSummaryBatches.toLocaleString()} summary batches from cache...`,
            loadStartedMs,
            phaseTimings: mainTimer.report(),
          });

          modeRef.current =
            cached.decompressedSize > 100 * 1024 * 1024 ? 'large' : 'small';
          aggregatesRef.current = {
            ...EMPTY_AGGREGATES,
            criticalPath: cached.criticalPath,
          };

          // Load columnar batches from IDB directly into the store.
          // Each batch is a BatchData (typed arrays); we just hold a
          // reference — no per-event materialization, no copying.
          const store = new EventSummaryStore();
          await mainTimer.timeAsync('cache_read_summary_batches', () =>
            loadCachedSummaryBatches(
              eventLogPath!,
              cached.totalSummaryBatches,
              data => {
                store.pushSealedBatch(data);
              },
            ),
          );

          if (cancelled) return;

          // For small cached logs, pre-load all chunks into memory. Read them
          // in parallel — sequential awaits made a slow chunk stall the whole
          // load with no progress signal.
          if (modeRef.current === 'small') {
            const totalChunks = cached.totalChunks;
            setState({
              status: 'loading',
              progress: `Loading ${totalChunks.toLocaleString()} log chunks from cache...`,
              loadStartedMs,
              phaseTimings: mainTimer.report(),
            });
            await mainTimer.timeAsync('cache_read_chunks', async () => {
              const chunks = await Promise.all(
                Array.from({length: totalChunks}, (_, i) =>
                  getCachedChunk(eventLogPath!, i),
                ),
              );
              for (const chunk of chunks) {
                if (chunk) batchBuffersRef.current.push(chunk);
              }
            });
          }

          if (cancelled) return;
          setState({
            status: 'loaded',
            rawSize: cached.rawSize,
            decompressedSize: cached.decompressedSize,
            totalEvents: store.length,
            fetchDecodeMs: Date.now() - loadStartedMs,
            phaseTimings: mainTimer.report(),
            summaries: store,
            getEventData,
            getEventDataAsync,
            getEventBytes,
            aggregates: {criticalPath: cached.criticalPath},
            getLazyAggregate,
            isLargeLog: modeRef.current === 'large',
          });
          return;
        }
      } catch {
        // Cache read failed — proceed with fresh download
      }

      if (cancelled) return;
      setState({
        status: 'loading',
        progress: 'Fetching event log...',
        loadStartedMs,
      });

      // Decide whether to use the parallel pipeline before kicking off the
      // fetch — instantiating the decoder early lets its worker spawn +
      // ping/pong handshake overlap with the fetch's TTFB so the
      // decompress worker is ready to start the moment the response body
      // is available.
      //
      // Default: parallel pipeline for all logs. Pass `?parallelDecode=0`
      // to fall back to the single-worker path (kept around as a safety
      // valve / for comparison).
      const useParallel = params.get('parallelDecode') !== '0';
      const parallelDecoder = useParallel
        ? new ParallelDecoder({numDecoders: 4})
        : null;
      // Set true when the worker pipeline reaches `done` — at that point
      // the dispatcher is still draining IDB writes and finalizing the
      // cache row in the background, and the orchestrator self-terminates
      // on `cacheWritten`. The finally block below must NOT terminate in
      // that window or the cache row stays incomplete.
      let decodeSucceeded = false;

      try {
        const res = await mainSpans.timeAsync('fetch', () =>
          backend.fetchEventLog(eventLogPath!),
        );

        if (cancelled) return;

        // The parallel path streams the response body straight into the
        // decompress worker (no main-thread buffering); the single-worker
        // path needs the full ArrayBuffer.
        const contentLength = parseInt(
          res.headers.get('content-length') ?? '0',
          10,
        );

        // Accumulate summaries in a columnar store as they arrive from the
        // worker. The temporary EventSummary[] objects from each batch get
        // GC'd after they're pushed into the store.
        const summariesStore = new EventSummaryStore();
        let rawSize = 0;
        let decompressedSize = 0;
        let isLarge = false;
        let latestPhaseTimings: PhaseTimings | undefined;
        let latestDecompressedBytes: number | undefined;
        let latestTimelineLanes: TimelineLane[] | undefined;

        // Build the user-facing progress text. We compose it on main so we
        // can append a live events/sec rate computed from the e2e elapsed
        // (worker progress messages don't have access to that clock).
        const composeProgress = (): string => {
          const elapsed = Date.now() - loadStartedMs;
          const events = summariesStore.length;
          const eventsStr = `${events.toLocaleString()} events`;
          const mbStr =
            latestDecompressedBytes != null && latestDecompressedBytes > 0
              ? ` (${(latestDecompressedBytes / (1024 * 1024)).toFixed(0)}MB decompressed)`
              : '';
          let rateStr = '';
          if (events > 0 && elapsed > 0) {
            const r = (events * 1000) / elapsed;
            rateStr =
              r >= 1000
                ? ` — ${(r / 1000).toFixed(1)}k events/s`
                : ` — ${r.toFixed(0)} events/s`;
          }
          return `Decoded ${eventsStr}${mbStr}${rateStr}`;
        };

        if (parallelDecoder) {
          if (!res.body) throw new Error('Response body missing');
          rawSize = contentLength;
          // Capture a 'await_decode' span on the main lane covering the
          // entire worker pipeline, so the timeline shows main blocked vs
          // worker activity.
          const awaitDecodeSpan = mainSpans.begin('await_decode');
          await new Promise<void>((resolve, reject) => {
            const decoder = parallelDecoder;
            decoder.decode(res.body!, eventLogPath!, contentLength, msg => {
              // Note: `done` resolves this Promise so main can transition
              // to 'loaded' immediately, but the dispatcher continues
              // draining IDB writes + running finalizeInvocation in the
              // background. The orchestrator self-terminates on
              // `cacheWritten`. The finally block at the bottom only
              // terminates if `decodeSucceeded` was never set — if it
              // ran here on the happy path we'd kill the worker
              // mid-finalize and leave `complete=false` on the
              // invocation row, which would invalidate the cache.
              if (cancelled) {
                decoder.terminate();
                return;
              }
              switch (msg.type) {
                case 'progress':
                  if (msg.phaseTimings) latestPhaseTimings = msg.phaseTimings;
                  if (msg.decompressedBytes != null)
                    latestDecompressedBytes = msg.decompressedBytes;
                  setState({
                    status: 'loading',
                    progress: composeProgress(),
                    decompressedBytes: latestDecompressedBytes,
                    phaseTimings: latestPhaseTimings,
                    eventsLoaded: summariesStore.length,
                    loadStartedMs,
                  });
                  break;
                case 'summaryBatch':
                  summariesStore.pushSealedBatch(msg.batch);
                  setState({
                    status: 'loading',
                    progress: composeProgress(),
                    decompressedBytes: latestDecompressedBytes,
                    phaseTimings: latestPhaseTimings,
                    eventsLoaded: summariesStore.length,
                    loadStartedMs,
                  });
                  break;
                case 'done':
                  decompressedSize = msg.decompressedSize;
                  // Prefer the dispatcher's actual received-bytes count
                  // over the Content-Length we originally captured —
                  // chunked responses report 0 / unknown there.
                  if (msg.receivedBytes > 0) rawSize = msg.receivedBytes;
                  isLarge = true;
                  modeRef.current = 'large';
                  latestPhaseTimings = msg.phaseTimings;
                  latestTimelineLanes = msg.timelineLanes;
                  aggregatesRef.current = {
                    ...EMPTY_AGGREGATES,
                    criticalPath: msg.criticalPath,
                  };
                  // Small log: heavy aggregates came inline — prefill
                  // the lazy cache so consumers don't round-trip through
                  // IDB. Large log: aggregates were already written to
                  // IDB by the dispatcher before posting `done`, so
                  // leave the cache empty and let `getLazyAggregate`
                  // serve from IDB on first access.
                  if (msg.inlineAggregates) {
                    lazyAggregateCacheRef.current = {
                      loadSpans: Promise.resolve(
                        msg.inlineAggregates.loadSpans,
                      ),
                      analysisSpans: Promise.resolve(
                        msg.inlineAggregates.analysisSpans,
                      ),
                      actionSpans: Promise.resolve(
                        msg.inlineAggregates.actionSpans,
                      ),
                    };
                  }
                  // Don't terminate here — the dispatcher continues
                  // draining IDB writes in the background and will send
                  // a `cacheWritten` event when done. The orchestrator
                  // self-terminates on receipt of that event.
                  decodeSucceeded = true;
                  resolve();
                  break;
                case 'cacheWritten':
                  // Background IDB writes finished. Refresh the loaded
                  // state's timeline + phase timings so the visualization
                  // shows the dispatcher's tail spans.
                  setState(prev => {
                    if (prev.status !== 'loaded') return prev;
                    // Keep the main lane (already drained when 'done'
                    // arrived); replace the worker lanes with refreshed
                    // ones that include the dispatcher's tail spans.
                    const mainLane = prev.timelineLanes?.[0];
                    const refreshedLanes = mainLane
                      ? [mainLane, ...msg.timelineLanes]
                      : msg.timelineLanes;
                    return {
                      ...prev,
                      phaseTimings: msg.phaseTimings,
                      timelineLanes: refreshedLanes,
                    };
                  });
                  break;
                case 'error':
                  decoder.terminate();
                  reject(new Error(msg.message));
                  break;
              }
            });
          });
          awaitDecodeSpan.end();
        } else {
          // Single-worker path needs the full body buffered up front.
          const arrayBuffer = await res.arrayBuffer();
          await new Promise<void>((resolve, reject) => {
            let worker: Worker;
            try {
              worker = new Worker(
                new URL(
                  '../../lib/streaming-decode-worker.ts',
                  import.meta.url,
                ),
              );
            } catch {
              reject(new Error('Failed to create worker'));
              return;
            }

            worker.onmessage = (e: MessageEvent<StreamingWorkerResponse>) => {
              if (cancelled) {
                worker.terminate();
                return;
              }
              const msg = e.data;

              switch (msg.type) {
                case 'progress':
                  if (msg.phaseTimings) latestPhaseTimings = msg.phaseTimings;
                  if (msg.decompressedBytes != null)
                    latestDecompressedBytes = msg.decompressedBytes;
                  setState({
                    status: 'loading',
                    progress: composeProgress(),
                    decompressedBytes: latestDecompressedBytes,
                    phaseTimings: latestPhaseTimings,
                    eventsLoaded: summariesStore.length,
                    loadStartedMs,
                  });
                  break;

                case 'summaryBatch': {
                  // The worker sent a sealed BatchData (typed arrays + pools).
                  // Push directly into the store — no per-event materialization.
                  summariesStore.pushSealedBatch(msg.batch);
                  setState({
                    status: 'loading',
                    progress: composeProgress(),
                    decompressedBytes: latestDecompressedBytes,
                    phaseTimings: latestPhaseTimings,
                    eventsLoaded: summariesStore.length,
                    loadStartedMs,
                  });
                  break;
                }

                case 'protoBuffer':
                  // Small-log path: keep uncompressed proto bytes in memory
                  batchBuffersRef.current.push(msg.data);
                  break;

                case 'done':
                  worker.terminate();
                  rawSize = msg.rawSize;
                  decompressedSize = msg.decompressedSize;
                  isLarge = msg.mode === 'large';
                  modeRef.current = msg.mode;
                  latestPhaseTimings = msg.phaseTimings;
                  resolve();
                  break;

                case 'error':
                  worker.terminate();
                  reject(new Error(msg.message));
                  break;
              }
            };

            worker.onerror = () => {
              worker.terminate();
              reject(new Error('Worker failed'));
            };

            worker.postMessage(
              {type: 'decode', compressed: arrayBuffer, eventLogPath},
              [arrayBuffer],
            );
          });
        }

        if (cancelled) return;

        const postDecodeSpan = mainSpans.begin('post_decode');

        // For large logs from the single-worker path, load criticalPath
        // from IDB (worker wrote it there). The parallel decoder posts
        // aggregates back directly with `done` so this lookup isn't
        // needed. Heavy aggregates (load/analysis/action) are read
        // lazily through the lazy-aggregate cache.
        if (isLarge && !useParallel) {
          try {
            const cachedInv = await getCachedInvocation(eventLogPath!);
            if (cachedInv) {
              aggregatesRef.current = {
                ...EMPTY_AGGREGATES,
                criticalPath: cachedInv.criticalPath,
              };
            }
          } catch {
            // Aggregates not available — treemaps/critical path won't work
          }
        }

        // Trim columnar buffers to actual size
        summariesStore.shrinkToFit();

        postDecodeSpan.end();

        // Prepend a 'main' lane to the worker timeline so the user can
        // see fetch / await_decode / post_decode in the same chart.
        // Main's spans use performance.now() directly so offsetMs = 0.
        const finalTimelineLanes = latestTimelineLanes
          ? [
              {
                workerName: 'main',
                epochMs: 0,
                offsetMs: 0,
                spans: mainSpans.drain(),
              },
              ...latestTimelineLanes,
            ]
          : undefined;

        setState({
          status: 'loaded',
          rawSize,
          decompressedSize,
          totalEvents: summariesStore.length,
          fetchDecodeMs: Date.now() - loadStartedMs,
          phaseTimings: latestPhaseTimings,
          timelineLanes: finalTimelineLanes,
          summaries: summariesStore,
          getEventData,
          getEventDataAsync,
          getEventBytes,
          aggregates: {criticalPath: aggregatesRef.current.criticalPath},
          getLazyAggregate,
          isLargeLog: isLarge,
        });
      } catch (e) {
        if (cancelled) return;
        setState({
          status: 'error',
          message: e instanceof Error ? e.message : 'Failed to load',
        });
      } finally {
        // Eagerly-constructed decoder may still be alive if we bailed out
        // before handing the body to it (cancellation, fetch error). Only
        // terminate when decode did NOT succeed — on the happy path the
        // dispatcher is still running its background tail (drain IDB
        // writes + `finalizeInvocation` to flip `complete=true`), and
        // the orchestrator self-terminates on `cacheWritten`. Killing
        // here on the happy path leaves the cache row with
        // `complete=false`, so the next load can't read it.
        if (!decodeSucceeded) {
          parallelDecoder?.terminate();
        }
      }
    }

    fetchAndDecode();
    return () => {
      cancelled = true;
    };
  }, [
    eventLogPath,
    backend,
    getEventData,
    getEventDataAsync,
    getEventBytes,
    getDecoder,
  ]);

  return (
    <EventLogContext.Provider value={state}>
      {children}
    </EventLogContext.Provider>
  );
}
