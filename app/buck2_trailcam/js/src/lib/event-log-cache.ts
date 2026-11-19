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
 * IndexedDB-backed cache for decoded event logs.
 *
 * Stores per-invocation:
 * - Metadata (summaries, aggregates, sizes, completion status)
 * - Raw proto byte chunks (uncompressed, ~4MB each)
 *
 * LRU eviction by invocation when total storage exceeds MAX_STORAGE_BYTES.
 * Works in both main thread and Web Workers.
 */

import Dexie, {type EntityTable} from 'dexie';
import type {EventSummary} from './event-log-decoder';
import type {AggregateData} from './streaming-collectors';
import type {BatchData} from './event-summary-store';
import type {CriticalPathData} from './critical-path';

// ============================================================================
// Schema
// ============================================================================

/**
 * Bump this whenever the cache format changes (EventSummary fields,
 * AggregateData shape, chunk layout, etc.). Cached entries with a
 * different version are treated as stale and discarded.
 *
 * v2: summaryBatches store BatchData (typed-array columnar) instead of
 *     EventSummary[] (avoids materializing 1M+ JS objects on read).
 * v3: CriticalPathCollector stores parsed CriticalPathData (PhaseGroup[]
 *     etc.) instead of raw protobuf bgInfo.
 * v4: CriticalPathEntry adds actionCategory field for action_execution
 *     entries (used to aggregate non-cache actions by category).
 * v5: Heavy aggregates (loadSpans/analysisSpans/actionSpans) split out
 *     of the invocation row into a separate `lazyAggregates` store so
 *     cache-hit reads stay fast. CachedInvocation only carries
 *     criticalPath inline now.
 * v6: Parallel pipeline now writes one IDB chunk row per decompress
 *     chunk (and one per cross-chunk straddler) instead of packing
 *     into ~4MB rows. Per-event (chunkIndex, offsetInChunk) lookups
 *     therefore use a different index space; previous v5 rows are
 *     stale.
 * v7: BatchData adds failedCol (Uint8Array) so the Actions tab can filter
 *     by action success/failure without decoding every event proto.
 * v8: Worker labelFromValue now unwraps the structured ConfiguredTargetLabel
 *     shape so action-execution events get their target_label populated in
 *     EventSummary (previously empty for actions).
 * v9: Parallel decoder worker now extracts and writes the failed bool into
 *     failedCol. Previously only the single-worker pipeline did, so caches
 *     written via the (default) parallel pipeline left failedCol filled with
 *     255 (= "unknown") for every action, breaking status classification on
 *     the Actions tab.
 * v10: CriticalPathEntry now carries the raw proto entry as `_raw` so the
 *      detailed critical path view can show a pretty JSON tooltip on hover.
 *      Cached criticalPath aggregates from earlier versions lack this field
 *      and would never trigger the tooltip.
 * v11: GENERIC_ENTRY_PHASE_OVERRIDES gained `build_key: execution`. Cached
 *      criticalPath aggregates need re-extraction so build_key entries land
 *      in the execution phase instead of being context-dependent.
 * v12: The v8 fix to unwrap structured ConfiguredTargetLabel had only ever
 *      been applied to the single-worker decoder. The parallel decoder
 *      worker (the default pipeline) still had the simpler labelFromValue
 *      that only handled string-shaped labels, so action_execution events
 *      cached via the parallel pipeline have empty targetLabel. Re-decode.
 * v13: AnalysisSpan adds retainedMemoryBytes (from
 *      AnalysisProfile.starlark_allocated_bytes) so the analysis treemap
 *      can size by retained Starlark heap. Older cached analysisSpans
 *      lack this field and would treat every target as 0 bytes.
 */
const CACHE_FORMAT_VERSION = 13;

/** Names of the aggregates stored lazily, one row each. */
export type LazyAggregateName = 'loadSpans' | 'analysisSpans' | 'actionSpans';

export interface CachedInvocation {
  /** The manifold event log path — unique key */
  eventLogPath: string;
  /** Cache format version — entries with a different version are discarded */
  formatVersion: number;
  lastAccessedMs: number;
  totalChunks: number;
  totalSummaryBatches: number;
  totalEvents: number;
  rawSize: number;
  decompressedSize: number;
  /** False until all chunks + summaries + aggregates are written */
  complete: boolean;
  /** Small / always-needed aggregates kept inline. The big lists
   *  (loadSpans/analysisSpans/actionSpans) live in `lazyAggregates`. */
  criticalPath: CriticalPathData | null;
}

export interface CachedLazyAggregate {
  /** Auto-incremented ID */
  id?: number;
  eventLogPath: string;
  name: LazyAggregateName;
  /** The aggregate value — loadSpans / analysisSpans / actionSpans
   *  arrays. Stored as a single row each; no chunking. */
  data: unknown;
}

export interface CachedChunk {
  /** Auto-incremented ID */
  id?: number;
  eventLogPath: string;
  chunkIndex: number;
  /** Raw uncompressed proto bytes for this chunk */
  data: ArrayBuffer;
}

export interface CachedSummaryBatch {
  /** Auto-incremented ID */
  id?: number;
  eventLogPath: string;
  batchIndex: number;
  /** Columnar batch data — typed arrays + per-batch string pools */
  data: BatchData;
}

// ============================================================================
// Database
// ============================================================================

const MAX_STORAGE_BYTES = 10 * 1024 * 1024 * 1024; // 10 GB

class EventLogCacheDB extends Dexie {
  invocations!: EntityTable<CachedInvocation, 'eventLogPath'>;
  chunks!: EntityTable<CachedChunk, 'id'>;
  summaryBatches!: EntityTable<CachedSummaryBatch, 'id'>;
  lazyAggregates!: EntityTable<CachedLazyAggregate, 'id'>;

  constructor() {
    super('trailcam-event-logs');
    // v2 → v3: summaryBatches.data field replaces summaries field
    this.version(3).stores({
      invocations: 'eventLogPath, lastAccessedMs',
      chunks: '++id, [eventLogPath+chunkIndex], eventLogPath',
      summaryBatches: '++id, [eventLogPath+batchIndex], eventLogPath',
    });
    // v3 → v4: add lazyAggregates store. Existing invocation rows are
    // invalidated by CACHE_FORMAT_VERSION bump; no migration of stored
    // aggregates is needed.
    this.version(4).stores({
      invocations: 'eventLogPath, lastAccessedMs',
      chunks: '++id, [eventLogPath+chunkIndex], eventLogPath',
      summaryBatches: '++id, [eventLogPath+batchIndex], eventLogPath',
      lazyAggregates: '++id, [eventLogPath+name], eventLogPath',
    });
    // v4 → v5: drop the redundant `eventLogPath` standalone index from
    // `chunks` / `summaryBatches` / `lazyAggregates`. The compound
    // `[eventLogPath+chunkIndex]` (etc.) index covers any
    // path-prefixed query because IDB compound indexes support range
    // scans on a key prefix. Removing the redundant index drops one
    // index update per insert, which is the dominant per-row cost
    // when bulk-writing thousands of chunk rows during decode.
    //
    // Queries that previously used `where('eventLogPath').equals(p)`
    // are rewritten to `where('[eventLogPath+...]').between([p, low], [p, high])`.
    this.version(5).stores({
      invocations: 'eventLogPath, lastAccessedMs',
      chunks: '++id, [eventLogPath+chunkIndex]',
      summaryBatches: '++id, [eventLogPath+batchIndex]',
      lazyAggregates: '++id, [eventLogPath+name]',
    });
  }
}

let dbInstance: EventLogCacheDB | null = null;

function getDB(): EventLogCacheDB {
  if (!dbInstance) {
    dbInstance = new EventLogCacheDB();
  }
  return dbInstance;
}

// ============================================================================
// Read operations
// ============================================================================

/**
 * Look up every primary key for rows whose compound index begins with
 * `eventLogPath`, using only the compound index (no separate
 * `eventLogPath` standalone index). Works for any compound index whose
 * first member is `eventLogPath`; the upper bound `[path, []]`
 * (exclusive) is greater than any `[path, primitive]` because IDB key
 * ordering treats arrays as larger than primitives.
 *
 * All three stores using this helper have `++id` (number) as their
 * primary key, so the returned array is `number[]`.
 */
async function pathPrimaryKeys<T extends {id?: number}>(
  table: EntityTable<T, 'id'>,
  compoundIndex: string,
  eventLogPath: string,
): Promise<number[]> {
  const keys = await table
    .where(compoundIndex)
    .between([eventLogPath], [eventLogPath, []], true, false)
    .primaryKeys();
  return keys as number[];
}

/**
 * Check if we have a complete cached invocation.
 * If found, updates lastAccessedMs.
 *
 * Records granular timings into the optional `timer` so callers can see
 * where the cache-read time goes (the IDB get, the lastAccessedMs
 * update, and the row's structured-clone byte size).
 */
export async function getCachedInvocation(
  eventLogPath: string,
  timer?: import('./phase-timer').PhaseTimer,
): Promise<CachedInvocation | null> {
  const db = getDB();
  const t0 = performance.now();
  const inv = await db.invocations.get(eventLogPath);
  const tGet = performance.now() - t0;
  if (timer) timer.add('cache_idb_get_invocation', tGet);
  if (!inv || !inv.complete || inv.formatVersion !== CACHE_FORMAT_VERSION) {
    // Missing, incomplete, or stale cache — delete and return null
    if (inv) {
      await deleteInvocation(eventLogPath);
    }
    return null;
  }

  const t1 = performance.now();
  await db.invocations.update(eventLogPath, {
    lastAccessedMs: Date.now(),
  });
  if (timer) timer.add('cache_idb_update_lastaccessed', performance.now() - t1);
  return inv;
}

/**
 * Stream summary batches from a cached invocation in batchIndex order.
 *
 * Reads batches in windows of `LOAD_WINDOW_SIZE` via `.toArray()` rather
 * than a `.each()` cursor — a cursor pays one IDB IPC roundtrip per
 * batch, which dominates for large logs (e.g. ~900 hops for 9M events).
 * Keeps `LOAD_IN_FLIGHT_WINDOWS` requests pipelined so consumer work
 * overlaps the next reads, while bounding peak memory to roughly
 * `LOAD_IN_FLIGHT_WINDOWS * LOAD_WINDOW_SIZE` batches in flight.
 *
 * Each yielded batch is a columnar BatchData (typed arrays + pools),
 * not a JS-object array. This avoids materializing 1M+ EventSummary
 * objects when loading a large cached invocation.
 */
const LOAD_WINDOW_SIZE = 50;
const LOAD_IN_FLIGHT_WINDOWS = 4;

export async function loadCachedSummaryBatches(
  eventLogPath: string,
  totalBatches: number,
  onBatch: (data: BatchData) => void,
): Promise<void> {
  const db = getDB();
  const windowCount = Math.ceil(totalBatches / LOAD_WINDOW_SIZE);

  const readWindow = (i: number): Promise<CachedSummaryBatch[]> => {
    const lo = i * LOAD_WINDOW_SIZE;
    const hi = Math.min((i + 1) * LOAD_WINDOW_SIZE, totalBatches);
    return db.summaryBatches
      .where('[eventLogPath+batchIndex]')
      .between([eventLogPath, lo], [eventLogPath, hi], false, true)
      .toArray();
  };

  const inFlight: Array<Promise<CachedSummaryBatch[]> | null> = [];
  let nextToIssue = 0;
  const prime = Math.min(LOAD_IN_FLIGHT_WINDOWS, windowCount);
  for (let i = 0; i < prime; i++) inFlight.push(readWindow(nextToIssue++));

  for (let consumed = 0; consumed < windowCount; consumed++) {
    const window = await inFlight.shift()!;
    if (nextToIssue < windowCount) inFlight.push(readWindow(nextToIssue++));
    for (const row of window) onBatch(row.data);
  }
}

/**
 * Read a single chunk's raw proto bytes from IndexedDB.
 */
export async function getCachedChunk(
  eventLogPath: string,
  chunkIndex: number,
): Promise<ArrayBuffer | null> {
  const db = getDB();
  const chunk = await db.chunks
    .where('[eventLogPath+chunkIndex]')
    .equals([eventLogPath, chunkIndex])
    .first();
  return chunk?.data ?? null;
}

// ============================================================================
// Write operations (used by the streaming worker)
// ============================================================================

/**
 * Per-eventLogPath background-cleanup promise. `startCachingInvocation`
 * snapshots stale primary keys synchronously and kicks off the actual
 * bulkDeletes as fire-and-forget; `finalizeInvocation` /
 * `writeWorkerResults` await this before flipping `complete=true` so a
 * concurrent reader never sees stale rows alongside fresh ones.
 *
 * Why deletes can run concurrently with new writes: stale rows are
 * snapshotted by primary key (`++id`) up front, so the bulkDelete set
 * is fixed. New chunk/batch writes get fresh auto-increment IDs that
 * are not in the snapshot.
 */
const pendingCleanups = new Map<string, Promise<void>>();

/**
 * Start caching a new invocation.
 *
 * Synchronously snapshots stale chunk/batch/lazy primary keys and
 * publishes the new (incomplete) invocation row so the caller can begin
 * writing immediately. The actual `bulkDelete`s of stale rows run in
 * the background — for a previously-decoded multi-GB log they're the
 * dominant cost here and would otherwise block the dispatcher's first
 * chunk write by hundreds of ms (visible in the timeline as a gap
 * between decompress producing chunks and dispatch processing them).
 *
 * Background cleanup completion is tracked via `pendingCleanups`;
 * `finalizeInvocation` and `writeWorkerResults` await it.
 */
export async function startCachingInvocation(
  eventLogPath: string,
  rawSize: number,
): Promise<void> {
  const db = getDB();
  const [chunkIds, batchIds, lazyIds] = await Promise.all([
    pathPrimaryKeys(db.chunks, '[eventLogPath+chunkIndex]', eventLogPath),
    pathPrimaryKeys(
      db.summaryBatches,
      '[eventLogPath+batchIndex]',
      eventLogPath,
    ),
    pathPrimaryKeys(db.lazyAggregates, '[eventLogPath+name]', eventLogPath),
  ]);
  await db.invocations.put({
    eventLogPath,
    formatVersion: CACHE_FORMAT_VERSION,
    lastAccessedMs: Date.now(),
    totalChunks: 0,
    totalSummaryBatches: 0,
    totalEvents: 0,
    rawSize,
    decompressedSize: 0,
    complete: false,
    criticalPath: null,
  });
  if (chunkIds.length > 0 || batchIds.length > 0 || lazyIds.length > 0) {
    const t0 = performance.now();
    const cleanup = Promise.all([
      chunkIds.length > 0 ? db.chunks.bulkDelete(chunkIds) : Promise.resolve(),
      batchIds.length > 0
        ? db.summaryBatches.bulkDelete(batchIds)
        : Promise.resolve(),
      lazyIds.length > 0
        ? db.lazyAggregates.bulkDelete(lazyIds)
        : Promise.resolve(),
    ])
      .then(() => {
        console.log(
          `[event-log-cache] background cleanup ${eventLogPath}: ` +
            `chunks=${chunkIds.length} batches=${batchIds.length} ` +
            `lazy=${lazyIds.length} took ${(performance.now() - t0).toFixed(0)}ms`,
        );
      })
      .catch((err: unknown) => {
        console.warn(
          '[event-log-cache] background cleanup failed:',
          eventLogPath,
          err,
        );
      });
    pendingCleanups.set(eventLogPath, cleanup);
  }
}

/**
 * Wait for any background cleanup kicked off by `startCachingInvocation`.
 * Called by finalize paths so cache reads after completion never see a
 * mix of old and new rows.
 */
export async function awaitCachingCleanup(eventLogPath: string): Promise<void> {
  const c = pendingCleanups.get(eventLogPath);
  if (!c) return;
  try {
    await c;
  } finally {
    if (pendingCleanups.get(eventLogPath) === c) {
      pendingCleanups.delete(eventLogPath);
    }
  }
}

/**
 * Write a sealed columnar BatchData to IndexedDB.
 */
export async function writeCachedSummaryBatch(
  eventLogPath: string,
  batchIndex: number,
  data: BatchData,
): Promise<void> {
  const db = getDB();
  await db.summaryBatches.put({
    eventLogPath,
    batchIndex,
    data,
  });
}

/**
 * Write a chunk of raw proto bytes to IndexedDB.
 */
export async function writeCachedChunk(
  eventLogPath: string,
  chunkIndex: number,
  data: ArrayBuffer,
): Promise<void> {
  const db = getDB();
  await db.chunks.put({
    eventLogPath,
    chunkIndex,
    data,
  });
}

/**
 * Write multiple chunks to IndexedDB in a single transaction. Per-row
 * transaction setup overhead dominates throughput when issuing many small
 * `put`s back-to-back, so bulkPut is dramatically faster (we've measured
 * the dispatcher steady-state idb_writes time drop sharply when batching).
 *
 * Order within the array doesn't matter for correctness — readers query
 * by `[eventLogPath+chunkIndex]` index, not insertion order.
 */
export async function writeCachedChunksBulk(
  rows: Array<{eventLogPath: string; chunkIndex: number; data: ArrayBuffer}>,
): Promise<void> {
  if (rows.length === 0) return;
  const db = getDB();
  await db.chunks.bulkPut(rows);
}

/**
 * Write multiple summary batches in a single transaction. Same rationale
 * as `writeCachedChunksBulk`.
 */
export async function writeCachedSummaryBatchesBulk(
  rows: Array<{eventLogPath: string; batchIndex: number; data: BatchData}>,
): Promise<void> {
  if (rows.length === 0) return;
  const db = getDB();
  await db.summaryBatches.bulkPut(rows);
}

/**
 * Write the three heavy aggregates (loadSpans, analysisSpans, actionSpans)
 * to their own `lazyAggregates` rows. Used by the parallel pipeline as a
 * standalone step so the dispatcher can write them before posting `done`
 * (avoiding the postMessage clone path), then finalize the invocation
 * row in the background.
 */
export async function writeLazyAggregates(
  eventLogPath: string,
  aggregates: AggregateData,
): Promise<void> {
  await Promise.all([
    writeLazyAggregate(eventLogPath, 'loadSpans', aggregates.loadSpans),
    writeLazyAggregate(eventLogPath, 'analysisSpans', aggregates.analysisSpans),
    writeLazyAggregate(eventLogPath, 'actionSpans', aggregates.actionSpans),
  ]);
}

/**
 * Mark an invocation complete: update the small inline criticalPath +
 * size/count metadata + run LRU eviction. Lazy aggregates must already
 * have been written via `writeLazyAggregates` (or `writeWorkerResults`).
 */
export async function finalizeInvocation(
  eventLogPath: string,
  criticalPath: CriticalPathData | null,
  totalChunks: number,
  totalSummaryBatches: number,
  totalEvents: number,
  rawSize: number,
  decompressedSize: number,
): Promise<void> {
  // Background cleanup of stale rows from the previous decode must finish
  // before we publish `complete: true` — otherwise a cache reader could
  // see both old and new rows for the same [eventLogPath+chunkIndex].
  await awaitCachingCleanup(eventLogPath);
  const db = getDB();
  await db.invocations.update(eventLogPath, {
    criticalPath,
    totalChunks,
    totalSummaryBatches,
    totalEvents,
    rawSize,
    decompressedSize,
    complete: true,
    lastAccessedMs: Date.now(),
  });
  await evictIfNeeded();
}

/**
 * Write aggregates and metadata at end-of-decode (single-worker pipeline).
 *
 * `criticalPath` is small and goes inline on the invocation row so it's
 * available immediately on cache hit (used by the Overview chart).
 *
 * `loadSpans`, `analysisSpans`, and `actionSpans` are written to their own
 * `lazyAggregates` rows so the cache-hit path doesn't pay the
 * structured-clone cost of millions of objects up front. Consumers fetch
 * them lazily via `readLazyAggregate(path, name)`.
 */
export async function writeWorkerResults(
  eventLogPath: string,
  aggregates: AggregateData,
  totalChunks: number,
  totalSummaryBatches: number,
  totalEvents: number,
  decompressedSize: number,
): Promise<void> {
  await writeLazyAggregates(eventLogPath, aggregates);
  // See finalizeInvocation — the background cleanup must complete before
  // we publish `complete: true`.
  await awaitCachingCleanup(eventLogPath);
  // Single-worker doesn't track an updated rawSize here — it was set
  // at startCachingInvocation time and stays accurate.
  const db = getDB();
  await db.invocations.update(eventLogPath, {
    criticalPath: aggregates.criticalPath,
    totalChunks,
    totalSummaryBatches,
    totalEvents,
    decompressedSize,
    complete: true,
    lastAccessedMs: Date.now(),
  });
  await evictIfNeeded();
}

/**
 * Write one lazy aggregate to its own IDB row. Replaces any existing row
 * for this `(eventLogPath, name)` pair.
 */
export async function writeLazyAggregate(
  eventLogPath: string,
  name: LazyAggregateName,
  data: unknown,
): Promise<void> {
  const db = getDB();
  // Replace existing row, if any.
  await db.lazyAggregates
    .where('[eventLogPath+name]')
    .equals([eventLogPath, name])
    .delete();
  await db.lazyAggregates.add({eventLogPath, name, data});
}

/**
 * Read a lazy aggregate from IDB. Returns `null` if missing (e.g. the
 * row was evicted or this aggregate was never written).
 */
export async function readLazyAggregate<T>(
  eventLogPath: string,
  name: LazyAggregateName,
): Promise<T | null> {
  const db = getDB();
  const row = await db.lazyAggregates
    .where('[eventLogPath+name]')
    .equals([eventLogPath, name])
    .first();
  return row ? (row.data as T) : null;
}

// ============================================================================
// Eviction
// ============================================================================

/**
 * Delete all data for an invocation.
 */
export async function deleteCachedInvocation(
  eventLogPath: string,
): Promise<void> {
  return deleteInvocation(eventLogPath);
}

async function deleteInvocation(eventLogPath: string): Promise<void> {
  const db = getDB();
  // Look up primary keys per table in parallel, then issue a single
  // `bulkDelete` per table (also parallel). For an invocation with many
  // thousands of chunk rows this is dramatically faster than the prior
  // serialized `where().delete()` chain — each .delete() walked an IDB
  // cursor and issued one `delete` per row, which is the right shape for
  // a small result set but pathological at scale (we've seen 2-minute
  // hangs on warm caches that hit this path during dispatcher init).
  const t0 = performance.now();
  const [chunkIds, batchIds, lazyIds] = await Promise.all([
    pathPrimaryKeys(db.chunks, '[eventLogPath+chunkIndex]', eventLogPath),
    pathPrimaryKeys(
      db.summaryBatches,
      '[eventLogPath+batchIndex]',
      eventLogPath,
    ),
    pathPrimaryKeys(db.lazyAggregates, '[eventLogPath+name]', eventLogPath),
  ]);
  const tKeys = performance.now();
  await Promise.all([
    chunkIds.length > 0 ? db.chunks.bulkDelete(chunkIds) : Promise.resolve(),
    batchIds.length > 0
      ? db.summaryBatches.bulkDelete(batchIds)
      : Promise.resolve(),
    lazyIds.length > 0
      ? db.lazyAggregates.bulkDelete(lazyIds)
      : Promise.resolve(),
    db.invocations.delete(eventLogPath),
  ]);
  const tDel = performance.now();
  // Log when there's anything substantial to clean — useful for diagnosing
  // cache-related stalls. Cheap when nothing to delete.
  if (chunkIds.length + batchIds.length + lazyIds.length > 0) {
    console.log(
      `[event-log-cache] deleteInvocation ${eventLogPath}: ` +
        `chunks=${chunkIds.length} batches=${batchIds.length} lazy=${lazyIds.length} ` +
        `keys=${(tKeys - t0).toFixed(0)}ms del=${(tDel - tKeys).toFixed(0)}ms`,
    );
  }
}

/**
 * Evict least-recently-accessed invocations until total storage is under the limit.
 */
async function evictIfNeeded(): Promise<void> {
  const db = getDB();

  // Estimate total storage: count chunks and their data sizes
  // (Dexie doesn't expose raw storage size, so we estimate from decompressedSize)
  const allInvocations = await db.invocations.toArray();
  let totalBytes = allInvocations.reduce(
    (s, inv) => s + inv.decompressedSize + inv.rawSize,
    0,
  );

  if (totalBytes <= MAX_STORAGE_BYTES) return;

  // Sort by last accessed, oldest first
  const sorted = allInvocations.sort(
    (a, b) => a.lastAccessedMs - b.lastAccessedMs,
  );

  for (const inv of sorted) {
    if (totalBytes <= MAX_STORAGE_BYTES) break;
    totalBytes -= inv.decompressedSize + inv.rawSize;
    await deleteInvocation(inv.eventLogPath);
  }
}

/**
 * Clear all cached data. Useful for debugging.
 */
export async function clearAllCachedData(): Promise<void> {
  const db = getDB();
  await db.chunks.clear();
  await db.summaryBatches.clear();
  await db.lazyAggregates.clear();
  await db.invocations.clear();
}
