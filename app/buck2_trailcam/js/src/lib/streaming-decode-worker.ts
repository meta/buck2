/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/**
 * Web Worker for large event log processing.
 *
 * Pipeline:
 * 1. Streaming zstd decompression (never holds full decompressed data)
 * 2. Varint message parsing (handles chunk boundaries)
 * 3. Per-event: protobuf decode → extract summary → feed collectors
 * 4. Accumulate raw bytes into ~4MB chunks → compress → transfer
 *
 * For small logs (<25MB compressed), falls back to the standard
 * all-at-once decompression path.
 */

// @ts-expect-error -- generated JS module
import {buck} from './proto/bundle.js';
import {VarintStreamParser} from './varint-stream-parser';
import {decompressZstdStreaming} from './zstd';
import {type EventSummary, StringPool} from './event-log-decoder';
import {
  LoadPackageCollector,
  AnalysisSpanCollector,
  ActionSpanCollector,
  CriticalPathCollector,
} from './streaming-collectors';
import {
  startCachingInvocation,
  writeCachedChunk,
  writeCachedSummaryBatch,
  writeWorkerResults,
} from './event-log-cache';
import {summariesToBatch, type BatchData} from './event-summary-store';
import {PhaseTimer, nowMs, type PhaseTimings} from './phase-timer';

const {CommandProgress} = buck.daemon;
const {Invocation} = buck.data;

const SMALL_LOG_THRESHOLD = 25 * 1024 * 1024; // 25MB compressed
const CHUNK_TARGET_SIZE = 4 * 1024 * 1024; // 4MB uncompressed per re-compressed chunk
const SUMMARY_BATCH_SIZE = 10000;

// ============================================================================
// Types
// ============================================================================

export interface StreamingWorkerRequest {
  type: 'decode';
  compressed: ArrayBuffer;
  /** Manifold path — used as IndexedDB key for caching */
  eventLogPath: string;
}

export type StreamingWorkerResponse =
  | {
      type: 'progress';
      message: string;
      decompressedBytes?: number;
      /** Cumulative phase-time breakdown so far. */
      phaseTimings?: PhaseTimings;
    }
  /** Columnar batch of summaries — main thread can pushSealedBatch directly */
  | {type: 'summaryBatch'; batch: BatchData}
  | {type: 'protoBuffer'; data: ArrayBuffer} // small-log path: uncompressed proto bytes
  | {
      type: 'done';
      rawSize: number;
      decompressedSize: number;
      totalEvents: number;
      mode: 'small' | 'large';
      chunkCount: number;
      /** Final phase-time breakdown. */
      phaseTimings: PhaseTimings;
    }
  | {type: 'error'; message: string};

// ============================================================================
// Event processing helpers (inlined from event-log-decoder to avoid deps)
// ============================================================================

const toObjectOpts = {longs: String, enums: String, defaults: false};

const SPAN_END_SKIP_KEYS = new Set(['stats', 'duration']);

function identifyEventType(data: Record<string, unknown>): string | undefined {
  for (const spanKey of ['spanStart', 'spanEnd', 'instant'] as const) {
    const span = data[spanKey] as Record<string, unknown> | undefined;
    if (!span) continue;
    const skip = spanKey === 'spanEnd' ? SPAN_END_SKIP_KEYS : undefined;
    for (const key of Object.keys(span)) {
      if (skip?.has(key)) continue;
      if (span[key] != null) return key;
    }
  }
  return undefined;
}

function identifyEventKind(data: Record<string, unknown>): string {
  if (data.spanStart) return 'spanStart';
  if (data.spanEnd) return 'spanEnd';
  if (data.instant) return 'instant';
  if (data.record) return 'record';
  return 'event';
}

function extractTimestampMs(ts: unknown): number | undefined {
  if (!ts || typeof ts !== 'object') return undefined;
  const obj = ts as Record<string, unknown>;
  return (
    Number(obj.seconds ?? 0) * 1000 +
    (typeof obj.nanos === 'number' ? obj.nanos / 1e6 : 0)
  );
}

function extractDurationMs(data: Record<string, unknown>): number | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const dur = spanEnd.duration as Record<string, unknown> | undefined;
  if (!dur) return undefined;
  return (
    Number(dur.seconds ?? 0) * 1000 +
    (typeof dur.nanos === 'number' ? dur.nanos / 1e6 : 0)
  );
}

function extractActionName(data: Record<string, unknown>): string | undefined {
  for (const key of ['spanStart', 'spanEnd'] as const) {
    const span = data[key] as Record<string, unknown> | undefined;
    if (!span) continue;
    const ae = span.actionExecution as Record<string, unknown> | undefined;
    if (!ae) continue;
    const name = ae.name as Record<string, unknown> | undefined;
    if (!name) continue;
    const category = name.category as string | undefined;
    const identifier = name.identifier as string | undefined;
    if (category || identifier)
      return [category, identifier].filter(Boolean).join(' ');
  }
  return undefined;
}

function extractExecutionKind(
  data: Record<string, unknown>,
): string | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const ae = spanEnd.actionExecution as Record<string, unknown> | undefined;
  return (ae?.executionKind as string) ?? undefined;
}

/**
 * For action-execution spanEnd events, return whether the action failed.
 * Returns undefined for any other event so the columnar store can mark the
 * row as "not applicable" instead of conflating with "succeeded".
 */
function extractActionFailed(
  data: Record<string, unknown>,
): boolean | undefined {
  const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
  if (!spanEnd) return undefined;
  const ae = spanEnd.actionExecution as Record<string, unknown> | undefined;
  if (!ae) return undefined;
  return !!(ae.failed as boolean);
}

function labelFromValue(val: unknown): string | undefined {
  if (typeof val === 'string') return val;
  if (val == null || typeof val !== 'object') return undefined;
  const obj = val as Record<string, unknown>;

  // Direct {package, name} shape (e.g. TargetLabel proto).
  if (typeof obj.package === 'string' || typeof obj.name === 'string') {
    const pkg = (obj.package as string) ?? '';
    const name = (obj.name as string) ?? '';
    if (pkg || name) return name ? `${pkg}:${name}` : pkg;
  }

  // Wrapped: {label: ...}. Used by ConfiguredTargetLabel and similar protos.
  if ('label' in obj) {
    const inner = obj.label;
    if (typeof inner === 'string') return inner;
    if (inner != null && typeof inner === 'object') {
      const lbl = inner as Record<string, unknown>;
      const pkg = (lbl.package as string) ?? '';
      const name = (lbl.name as string) ?? '';
      if (pkg || name) return name ? `${pkg}:${name}` : pkg;
    }
  }

  return undefined;
}

function extractTargetLabel(data: Record<string, unknown>): string | undefined {
  for (const key of ['spanStart', 'spanEnd'] as const) {
    const span = data[key] as Record<string, unknown> | undefined;
    if (!span) continue;
    const analysis = span.analysis as Record<string, unknown> | undefined;
    if (analysis?.target) {
      const label = labelFromValue(analysis.target);
      if (label) return label;
    }
    if (analysis?.standardTarget) {
      const label = labelFromValue(analysis.standardTarget);
      if (label) return label;
    }
    const ae = span.actionExecution as Record<string, unknown> | undefined;
    if (ae) {
      const k = ae.key as Record<string, unknown> | undefined;
      if (k?.targetLabel) {
        const label = labelFromValue(k.targetLabel);
        if (label) return label;
      }
    }
  }
  const instant = data.instant as Record<string, unknown> | undefined;
  if (instant?.testResult) {
    const tr = instant.testResult as Record<string, unknown>;
    if (tr.targetLabel) return labelFromValue(tr.targetLabel);
  }
  return undefined;
}

function parseSpanId(s: string | undefined): number | undefined {
  if (!s || s === '0') return undefined;
  return Number(s);
}

// ============================================================================
// Main worker logic
// ============================================================================

self.onmessage = async (e: MessageEvent<StreamingWorkerRequest>) => {
  if (e.data.type !== 'decode') return;

  try {
    const compressed = new Uint8Array(e.data.compressed);
    const rawSize = compressed.length;
    const eventLogPath = e.data.eventLogPath;
    const isLarge = rawSize >= SMALL_LOG_THRESHOLD;

    if (isLarge) {
      await processLargeLog(compressed, rawSize, eventLogPath);
    } else {
      await processSmallLog(compressed, rawSize, eventLogPath);
    }
  } catch (err) {
    self.postMessage({
      type: 'error',
      message: err instanceof Error ? err.message : 'Worker decode failed',
    } as StreamingWorkerResponse);
  }
};

// ============================================================================
// Small log path (current behavior, all-at-once decompress)
// ============================================================================

async function processSmallLog(
  compressed: Uint8Array,
  rawSize: number,
  eventLogPath: string,
) {
  const {decompressZstd} = await import('./zstd');
  const {decodeEventLogWithSummaries} = await import('./event-log-decoder');
  const timer = new PhaseTimer();

  self.postMessage({
    type: 'progress',
    message: 'Decompressing...',
  } as StreamingWorkerResponse);
  const decompressed = await timer.timeAsync('decompress', () =>
    decompressZstd(compressed),
  );
  const decompressedSize = decompressed.length;

  self.postMessage({
    type: 'progress',
    message: 'Decoding events...',
  } as StreamingWorkerResponse);

  // Start IDB caching
  await timer.timeAsync('idb_writes', () =>
    startCachingInvocation(eventLogPath, rawSize),
  );

  let totalEvents = 0;
  let chunkIndex = 0;
  let summaryBatchIdx = 0;
  // The generator does varint parsing + per-message decode + summary
  // extraction in one pass; in the small-log path we time it as a single
  // bucket. The large-log path breaks it down further.
  for (const batch of decodeEventLogWithSummaries(
    decompressed,
    SUMMARY_BATCH_SIZE,
  )) {
    // Convert to columnar BatchData (used for both post and IDB write)
    const data = timer.time('summary_extract', () =>
      summariesToBatch(batch.summaries),
    );

    // Post proto buffer to main thread (for in-memory access on small logs)
    self.postMessage({
      type: 'protoBuffer',
      data: batch.protoBuffer,
    } as StreamingWorkerResponse);
    self.postMessage({
      type: 'summaryBatch',
      batch: data,
    } as StreamingWorkerResponse);

    // Write chunk + summary batch to IDB for future cache hits
    await timer.timeAsync('idb_writes', () =>
      writeCachedChunk(eventLogPath, chunkIndex, batch.protoBuffer),
    );
    await timer.timeAsync('idb_writes', () =>
      writeCachedSummaryBatch(eventLogPath, summaryBatchIdx, data),
    );
    chunkIndex++;
    summaryBatchIdx++;

    totalEvents += batch.summaries.length;
  }

  // Finalize IDB cache
  await timer.timeAsync('idb_writes', () =>
    writeWorkerResults(
      eventLogPath,
      {loadSpans: [], actionSpans: [], analysisSpans: [], criticalPath: null},
      chunkIndex,
      summaryBatchIdx,
      totalEvents,
      decompressedSize,
    ),
  );

  self.postMessage({
    type: 'done',
    rawSize,
    decompressedSize,
    totalEvents,
    mode: 'small',
    chunkCount: chunkIndex,
    phaseTimings: timer.report(),
  } as StreamingWorkerResponse);
}

// ============================================================================
// Large log path (streaming decompress → IDB storage)
// ============================================================================

async function processLargeLog(
  compressed: Uint8Array,
  rawSize: number,
  eventLogPath: string,
) {
  const pool = new StringPool();
  const parser = new VarintStreamParser();
  const timer = new PhaseTimer();

  // Collectors for aggregate data
  // Action span collector is expensive for very large logs (>100MB compressed)
  // — skip it and let the user compute on demand if needed
  const isVeryLarge = rawSize > 100 * 1024 * 1024;
  const loadCollector = new LoadPackageCollector();
  const analysisCollector = new AnalysisSpanCollector();
  const actionCollector = isVeryLarge ? null : new ActionSpanCollector();
  const critPathCollector = new CriticalPathCollector();
  const collectors: import('./streaming-collectors').StreamingCollector[] = [
    loadCollector,
    analysisCollector,
    actionCollector,
    critPathCollector,
  ].filter(Boolean) as import('./streaming-collectors').StreamingCollector[];

  let index = 0;
  let isFirst = true;
  let decompressedBytes = 0;

  // Chunk accumulator (written to IndexedDB, not re-compressed)
  let chunkIndex = 0;
  let chunkParts: Uint8Array[] = [];
  let chunkSize = 0;

  // Summary batch accumulator
  let summaryBatch: EventSummary[] = [];
  let summaryBatchIndex = 0;
  let offsetInChunk = 0;

  // Pending IDB writes — kicked off fire-and-forget so they overlap with the
  // next chunk's decompression and decode rather than blocking on the
  // sequential await chain. Bounded to avoid unlimited memory growth and to
  // surface IDB backpressure into the timing buckets.
  let pendingSummaryWrites: Promise<void>[] = [];
  let pendingChunkWrites: Promise<void>[] = [];
  const MAX_PENDING_SUMMARY_WRITES = 8;
  const MAX_PENDING_CHUNK_WRITES = 4;

  // Start IDB caching
  await timer.timeAsync('idb_writes', () =>
    startCachingInvocation(eventLogPath, rawSize),
  );

  async function flushChunk() {
    if (chunkParts.length === 0) return;

    // Concatenate parts into a single ArrayBuffer (synchronous).
    const totalLen = chunkParts.reduce((s, p) => s + p.length, 0);
    const uncompressed = new Uint8Array(totalLen);
    let off = 0;
    for (const part of chunkParts) {
      uncompressed.set(part, off);
      off += part.length;
    }

    // Bookkeeping advances synchronously so the next decoded message gets
    // the right batchIndex/offset.
    const writeIdx = chunkIndex;
    chunkIndex++;
    chunkParts = [];
    chunkSize = 0;
    offsetInChunk = 0;

    // Kick the IDB write fire-and-forget — runs concurrently with the next
    // chunk's decompression/decode. Backpressure: if too many writes are
    // already in flight, await one before letting the new one queue up.
    if (pendingChunkWrites.length >= MAX_PENDING_CHUNK_WRITES) {
      await timer.timeAsync('idb_writes', () =>
        Promise.race(pendingChunkWrites).then(() => undefined),
      );
    }
    const writePromise = writeCachedChunk(
      eventLogPath,
      writeIdx,
      uncompressed.buffer,
    );
    pendingChunkWrites.push(writePromise);
    // Self-clean once resolved so the array doesn't grow without bound.
    void writePromise.then(() => {
      const i = pendingChunkWrites.indexOf(writePromise);
      if (i >= 0) pendingChunkWrites.splice(i, 1);
    });
  }

  async function flushSummaries() {
    if (summaryBatch.length === 0) return;
    // Convert EventSummary[] to columnar BatchData once. We use the same
    // BatchData both for IDB write and for posting to the main thread —
    // the structured clone of typed arrays is much cheaper than cloning
    // ~10K JS objects.
    const data = timer.time('summary_extract', () =>
      summariesToBatch(summaryBatch),
    );
    const batchIdx = summaryBatchIndex;
    summaryBatchIndex++;
    summaryBatch = [];

    // Post to main thread (clones the BatchData; typed arrays clone fast)
    self.postMessage({
      type: 'summaryBatch',
      batch: data,
    } as StreamingWorkerResponse);

    // Fire-and-forget IDB write with bounded backpressure. Time spent
    // waiting on the write counts toward `idb_writes` only when we
    // actually block (i.e. when too many writes are already in flight).
    if (pendingSummaryWrites.length >= MAX_PENDING_SUMMARY_WRITES) {
      await timer.timeAsync('idb_writes', () =>
        Promise.race(pendingSummaryWrites).then(() => undefined),
      );
    }
    const writePromise = writeCachedSummaryBatch(eventLogPath, batchIdx, data);
    pendingSummaryWrites.push(writePromise);
    void writePromise.then(() => {
      const i = pendingSummaryWrites.indexOf(writePromise);
      if (i >= 0) pendingSummaryWrites.splice(i, 1);
    });
  }

  /** Drain ALL pending writes — used at the very end. */
  async function drainAllPendingWrites() {
    if (pendingSummaryWrites.length === 0 && pendingChunkWrites.length === 0)
      return;
    await timer.timeAsync('idb_writes', async () => {
      await Promise.all(pendingSummaryWrites);
      await Promise.all(pendingChunkWrites);
    });
    pendingSummaryWrites = [];
    pendingChunkWrites = [];
  }

  function processMessage(msgBytes: Uint8Array) {
    let summary: EventSummary | null = null;

    try {
      if (isFirst) {
        isFirst = false;
        // Time the proto decode + toObject for the Invocation envelope.
        const t0 = nowMs();
        const invocation = Invocation.decode(msgBytes);
        const obj = Invocation.toObject(invocation, toObjectOpts);
        timer.add('proto_decode', nowMs() - t0);
        const ts = obj.startTime ?? undefined;
        const t1 = nowMs();
        summary = {
          index,
          type: pool.intern('invocation')!,
          timestampMs: extractTimestampMs(ts),
          batchIndex: chunkIndex,
          offsetInBatch: offsetInChunk,
          lengthInBatch: msgBytes.length,
        };
        timer.add('summary_extract', nowMs() - t1);
        const t2 = nowMs();
        for (const c of collectors) c.processEvent(summary, obj);
        timer.add('collectors_total', nowMs() - t2);
      } else {
        const t0 = nowMs();
        const progress = CommandProgress.decode(msgBytes);
        const progressObj = CommandProgress.toObject(progress, toObjectOpts);
        timer.add('proto_decode', nowMs() - t0);

        if (progressObj.event) {
          const evt = progressObj.event;
          const t1 = nowMs();
          const kind = identifyEventKind(evt);
          const eventType = identifyEventType(evt);
          summary = {
            index,
            type: pool.intern(kind)!,
            eventType: pool.intern(eventType),
            timestampMs: extractTimestampMs(evt.timestamp ?? undefined),
            spanId: parseSpanId(evt.spanId),
            parentId: parseSpanId(evt.parentId),
            durationMs: extractDurationMs(evt),
            actionName: pool.intern(extractActionName(evt)),
            executionKind: pool.intern(extractExecutionKind(evt)),
            targetLabel: pool.intern(extractTargetLabel(evt)),
            failed: extractActionFailed(evt),
            batchIndex: chunkIndex,
            offsetInBatch: offsetInChunk,
            lengthInBatch: msgBytes.length,
          };
          timer.add('summary_extract', nowMs() - t1);
          const t2 = nowMs();
          for (const c of collectors) c.processEvent(summary, evt);
          timer.add('collectors_total', nowMs() - t2);
        } else if (progressObj.result) {
          summary = {
            index,
            type: pool.intern('result')!,
            batchIndex: chunkIndex,
            offsetInBatch: offsetInChunk,
            lengthInBatch: msgBytes.length,
          };
        } else if (progressObj.partialResult) {
          summary = {
            index,
            type: pool.intern('partial_result')!,
            batchIndex: chunkIndex,
            offsetInBatch: offsetInChunk,
            lengthInBatch: msgBytes.length,
          };
        }
      }
    } catch {
      summary = {
        index,
        type: pool.intern('unknown')!,
        batchIndex: chunkIndex,
        offsetInBatch: offsetInChunk,
        lengthInBatch: msgBytes.length,
      };
    }

    if (summary) {
      summaryBatch.push(summary);

      // Accumulate raw proto bytes for IDB chunk
      // (msgBytes is already a copy from the varint parser)
      chunkParts.push(msgBytes);
      offsetInChunk += msgBytes.length;
      chunkSize += msgBytes.length;
    }

    index++;
  }

  // Stream through the compressed data
  self.postMessage({
    type: 'progress',
    message: 'Streaming decompression...',
  } as StreamingWorkerResponse);

  // Manual iteration so we can explicitly release each decompressed chunk
  // before requesting the next one (avoids holding the previous chunk alive
  // while waiting for IDB writes etc.)
  const iter = decompressZstdStreaming(compressed)[Symbol.asyncIterator]();
  // eslint-disable-next-line no-constant-condition
  while (true) {
    const decompT0 = nowMs();
    const result = await iter.next();
    timer.add('decompress', nowMs() - decompT0);
    if (result.done) break;
    let decompressedChunk: Uint8Array | null = result.value;
    decompressedBytes += decompressedChunk.length;

    // Parse complete protobuf messages from this chunk.
    // The parser internally yields slice() copies of each message so the
    // decompressed chunk is not pinned by the message references.
    // We measure the varint parser as the wall time of the parse iterator
    // *minus* the per-message processing time it dispatches into; the loop
    // body's processMessage already attributes its own time elsewhere.
    const parseLoopT0 = nowMs();
    let perMessageMs = 0;
    for (const msgBytes of parser.parse(decompressedChunk, timer)) {
      // We time processMessage + any pending flush awaits together so that
      // the residual `varint_parse` bucket only attributes parser-iterator
      // overhead. The flushes have their own timing internally
      // (`idb_writes`, `summary_extract`).
      const procT0 = nowMs();
      processMessage(msgBytes);
      if (chunkSize >= CHUNK_TARGET_SIZE) {
        await flushChunk();
      }
      if (summaryBatch.length >= SUMMARY_BATCH_SIZE) {
        await flushSummaries();
      }
      perMessageMs += nowMs() - procT0;
    }
    timer.add('varint_parse', nowMs() - parseLoopT0 - perMessageMs);

    // Explicitly release the decompressed chunk reference before the next iter
    decompressedChunk = null;

    self.postMessage({
      type: 'progress',
      message: `Decoded ${index} events (${(decompressedBytes / (1024 * 1024)).toFixed(0)}MB decompressed)...`,
      decompressedBytes,
      phaseTimings: timer.report(),
    } as StreamingWorkerResponse);

    // Yield to event loop for GC. Pending IDB writes keep running in
    // the background and will be drained at the very end.
    await new Promise(r => setTimeout(r, 0));
  }

  // Handle any remaining bytes
  const remainder = parser.flush();
  if (remainder && remainder.length > 0) {
    // Truncated last message — ignore
  }

  // Flush final batch + chunk, then drain everything still in flight.
  await flushSummaries();
  await flushChunk();
  await drainAllPendingWrites();

  // Finalize collectors
  loadCollector.finalize();

  const aggregates = {
    loadSpans: loadCollector.results,
    actionSpans: actionCollector?.results ?? [],
    analysisSpans: analysisCollector.results,
    criticalPath: critPathCollector.result,
  };

  // Write aggregates + metadata to IDB, mark complete
  await timer.timeAsync('idb_writes', () =>
    writeWorkerResults(
      eventLogPath,
      aggregates,
      chunkIndex,
      summaryBatchIndex,
      index,
      decompressedBytes,
    ),
  );

  self.postMessage({
    type: 'done',
    rawSize,
    decompressedSize: decompressedBytes,
    totalEvents: index,
    mode: 'large',
    chunkCount: chunkIndex,
    phaseTimings: timer.report(),
  } as StreamingWorkerResponse);
}
