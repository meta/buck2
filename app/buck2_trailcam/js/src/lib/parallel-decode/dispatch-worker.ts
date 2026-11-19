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
 * Dispatch worker.
 *
 * Per decompress chunk:
 *
 *   1. Parse the chunk in place (`VarintStreamParser.parseInPlace`) —
 *      yields message offsets within the chunk plus an optional
 *      reassembled "leading" message that straddled the previous
 *      chunk boundary.
 *   2. Write the chunk to IDB as its own row (1 IDB clone per chunk;
 *      structured-cloned by IDB itself, no manual memcpy here).
 *   3. If a leading straddler exists, write it to its own tiny IDB
 *      row (rare).
 *   4. Append a `ChunkDescriptor` to the current decoder accumulator
 *      (round-robin per chunk across decoders). When the accumulator
 *      reaches a target byte/message count, dispatch by transferring
 *      every chunk buffer in the batch to the target decoder — no
 *      packing, no per-message memcpy.
 *
 * Compared with the previous design, the dispatcher does *zero*
 * per-message memcpys; the only large memory traffic is the IDB
 * write's structured clone, which is unavoidable. See
 * `plans/parallel-decode-aggregates-via-idb.md` for context.
 */

import {VarintStreamParser} from '../varint-stream-parser';
import {
  finalizeInvocation,
  startCachingInvocation,
  writeCachedChunksBulk,
  writeCachedSummaryBatchesBulk,
  writeLazyAggregates,
} from '../event-log-cache';
import {
  LoadPackageCollector,
  type AggregateData,
} from '../streaming-collectors';
import {PhaseTimer, SpanRecorder, nowMs, type Span} from '../phase-timer';
import {type BatchData, SUMMARY_BATCH_SIZE} from '../event-summary-store';
import type {
  ChunkDescriptor,
  DecodeBatchRequest,
  DispatchRequest,
  DispatchResponse,
  PartialAggregates,
} from './types';

const post = (msg: DispatchResponse, transfer?: Transferable[]) => {
  if (transfer && transfer.length)
    (self as unknown as Worker).postMessage(msg, transfer);
  else (self as unknown as Worker).postMessage(msg);
};

// Each decompress chunk → its own IDB row. Decoder batches are sized to
// ~1MB or ~10K messages worth of decompress chunks (whichever fills
// first); below those thresholds we batch multiple chunks per decoder
// dispatch to amortize postMessage overhead.
const DECODER_BATCH_TARGET_BYTES = 1 * 1024 * 1024;
const DECODER_BATCH_TARGET_MESSAGES = 10000;

// Per-IDB-write transaction setup is the dominant cost for many small
// `put`s back-to-back, so we accumulate `CHUNK_BULK_SIZE` chunk rows
// (and `SUMMARY_BULK_SIZE` summary-batch rows) into a single bulkPut.
// The threshold trades latency-to-IDB for fewer transactions; values
// chosen so a typical multi-GB log doesn't sit in the buffer for long.
const CHUNK_BULK_SIZE = 8;
const SUMMARY_BULK_SIZE = 4;
// Memory-based backpressure: the dispatcher pipeline runs without
// awaiting individual IDB writes, but if buffered + in-flight chunk
// bytes grow past this threshold we pause chunk ingest until the queue
// drains. Sized for "a lot but not unbounded" on multi-GB logs — cap
// memory growth without commonly stalling the pipeline.
const CHUNK_BACKPRESSURE_BYTES = 128 * 1024 * 1024;
const SUMMARY_BACKPRESSURE_BYTES = 64 * 1024 * 1024;

let initParams: {
  numDecoders: number;
  eventLogPath: string;
  compressedSize: number;
} | null = null;

const timer = new PhaseTimer();
const parser = new VarintStreamParser();
const spanRecorder = new SpanRecorder();

let totalEvents = 0;
/** Monotonic IDB chunk-row index. Bumped by 1 for each in-chunk row
 *  written, plus 1 for each leading-straddler row written. */
let nextIdbChunkIndex = 0;

/** Per-decoder accumulator state — collects ChunkDescriptors until a
 *  byte/message threshold, then dispatches in one postMessage. */
interface DecoderAccum {
  chunks: ChunkDescriptor[];
  totalBytes: number;
  totalMessages: number;
  startEventIndex: number;
  hasInvocationFirst: boolean;
}

let decoderAccums: DecoderAccum[] = [];
let nextDecoder = 0;
let nextBatchId = 0;

/** Batches in-flight: batchId → completed result, awaiting in-order emit. */
const completedBatches = new Map<number, BatchData>();
let nextEmitBatchId = 0;

/** Flat list of every decoder result's partial aggregates. Merged once at
 *  the end — concat-on-each-result is O(N²) and will eat 50s+ on
 *  hundred-batch logs. */
const partials: PartialAggregates[] = [];

/** Buffered chunk rows awaiting their next bulkPut flush. */
let chunkWriteBuffer: Array<{
  eventLogPath: string;
  chunkIndex: number;
  data: ArrayBuffer;
}> = [];
/** Bulk chunk-write Promises currently in flight against IDB. */
let pendingChunkBulks: Promise<void>[] = [];
/** Sum of bytes in `chunkWriteBuffer` plus all in-flight chunk bulkPuts.
 *  Drives memory-based backpressure for the chunk write pipeline. */
let pendingChunkBytes = 0;
/** Buffered summary-batch rows awaiting their next bulkPut flush. */
let summaryWriteBuffer: Array<{
  eventLogPath: string;
  batchIndex: number;
  data: BatchData;
}> = [];
/** Bulk summary-batch-write Promises currently in flight against IDB. */
let pendingSummaryBulks: Promise<void>[] = [];
/** Approximate sum of bytes in `summaryWriteBuffer` plus all in-flight
 *  summary bulkPuts. BatchData is dominated by typed arrays so we sum
 *  their byteLengths; pool/index overhead is small. */
let pendingSummaryBytes = 0;

let decompressionDone = false;
let decompressedBytes = 0;
let receivedBytes = 0;
let outstandingDecoderBatches = 0;
let resolveAllDone: (() => void) | null = null;

/**
 * Below this threshold, the dispatcher includes the heavy aggregates
 * (loadSpans/analysisSpans/actionSpans) inline in the `done` payload so
 * the main thread can prefill its lazy-aggregate cache without a second
 * IDB round-trip.
 *
 * Above the threshold, those aggregates are written to IDB *before*
 * `done` is posted, and `done` carries only the small criticalPath. This
 * is required because at multi-million-event scale the structured-clone
 * cost of postMessaging the merged aggregates exceeds the renderer's
 * heap budget (we observed `DataCloneError: out of memory` on a
 * ~4GB-decompressed log). See plans/parallel-decode-aggregates-via-idb.md.
 */
const INLINE_AGGREGATES_COMPRESSED_BYTES = 25 * 1024 * 1024;

function freshAccum(eventIndex: number): DecoderAccum {
  return {
    chunks: [],
    totalBytes: 0,
    totalMessages: 0,
    startEventIndex: eventIndex,
    hasInvocationFirst: eventIndex === 0,
  };
}

/**
 * Buffer a chunk row for the next bulkPut. Pure synchronous bookkeeping
 * — no awaits and no IDB calls. When the buffer fills, kicks off a
 * fire-and-forget bulkPut. The dispatcher pipeline never blocks on IDB
 * here; backpressure is applied separately via `awaitChunkBackpressure`
 * at the start of each chunk-ingest, only kicking in when buffered +
 * in-flight bytes exceed `CHUNK_BACKPRESSURE_BYTES`.
 */
function enqueueChunkWrite(idbChunkIdx: number, buffer: ArrayBuffer): void {
  chunkWriteBuffer.push({
    eventLogPath: initParams!.eventLogPath,
    chunkIndex: idbChunkIdx,
    data: buffer,
  });
  pendingChunkBytes += buffer.byteLength;
  if (chunkWriteBuffer.length >= CHUNK_BULK_SIZE) {
    flushChunkBuffer();
  }
}

/** Last completed chunk-bulkPut wall time (ms). Recorded as a span attr
 *  on the next chunk that observes it, so a slow IDB write surfaces on
 *  the dispatcher lane without needing a separate, overlapping span. */
let lastChunkBulkMs = 0;
let lastChunkBulkRows = 0;

/** Flush whatever is currently in the chunk buffer as one bulkPut.
 *  Synchronous: starts the bulkPut and tracks it; does not await. */
function flushChunkBuffer(): void {
  if (chunkWriteBuffer.length === 0) return;
  const rows = chunkWriteBuffer;
  chunkWriteBuffer = [];
  const bytes = rows.reduce((acc, r) => acc + r.data.byteLength, 0);
  const t0 = nowMs();
  const writePromise = writeCachedChunksBulk(rows);
  pendingChunkBulks.push(writePromise);
  void writePromise.then(() => {
    lastChunkBulkMs = nowMs() - t0;
    lastChunkBulkRows = rows.length;
    pendingChunkBytes -= bytes;
    timer.add('idb_writes', lastChunkBulkMs);
    const i = pendingChunkBulks.indexOf(writePromise);
    if (i >= 0) pendingChunkBulks.splice(i, 1);
  });
}

/**
 * If buffered + in-flight chunk bytes exceed the backpressure threshold,
 * await in-flight bulks until we drop back below. Returns the wait time
 * (0 if no wait was needed) so callers can record it as a span attr.
 */
async function awaitChunkBackpressure(): Promise<number> {
  if (pendingChunkBytes <= CHUNK_BACKPRESSURE_BYTES) return 0;
  const t0 = nowMs();
  while (
    pendingChunkBytes > CHUNK_BACKPRESSURE_BYTES &&
    pendingChunkBulks.length > 0
  ) {
    await timer.timeAsync('idb_writes', () =>
      Promise.race(pendingChunkBulks).then(() => undefined),
    );
  }
  return nowMs() - t0;
}

/**
 * Buffer a summary-batch row for the next bulkPut. Pure sync bookkeeping.
 * Backpressure on memory is applied via `awaitSummaryBackpressure` from
 * the chunk-ingest path (the only place we can safely await).
 */
function enqueueSummaryWrite(batchIndex: number, data: BatchData): void {
  summaryWriteBuffer.push({
    eventLogPath: initParams!.eventLogPath,
    batchIndex,
    data,
  });
  pendingSummaryBytes += approxBatchDataBytes(data);
  if (summaryWriteBuffer.length >= SUMMARY_BULK_SIZE) {
    flushSummaryBuffer();
  }
}

function approxBatchDataBytes(d: BatchData): number {
  // Sum the typed arrays — pool/index overhead is small relative to columns.
  let total = 0;
  for (const v of Object.values(d) as unknown[]) {
    if (v && typeof v === 'object' && 'byteLength' in v) {
      total += (v as ArrayBufferView).byteLength;
    }
  }
  return total;
}

let lastSummaryBulkMs = 0;
let lastSummaryBulkRows = 0;

/** Flush whatever is currently in the summary buffer as one bulkPut.
 *  Synchronous: starts the bulkPut and tracks it; does not await. */
function flushSummaryBuffer(): void {
  if (summaryWriteBuffer.length === 0) return;
  const rows = summaryWriteBuffer;
  summaryWriteBuffer = [];
  const bytes = rows.reduce((acc, r) => acc + approxBatchDataBytes(r.data), 0);
  const t0 = nowMs();
  const writePromise = writeCachedSummaryBatchesBulk(rows);
  pendingSummaryBulks.push(writePromise);
  void writePromise.then(() => {
    lastSummaryBulkMs = nowMs() - t0;
    lastSummaryBulkRows = rows.length;
    pendingSummaryBytes -= bytes;
    timer.add('idb_writes', lastSummaryBulkMs);
    const i = pendingSummaryBulks.indexOf(writePromise);
    if (i >= 0) pendingSummaryBulks.splice(i, 1);
  });
}

/** Like `awaitChunkBackpressure` but for summary bulks. */
async function awaitSummaryBackpressure(): Promise<number> {
  if (pendingSummaryBytes <= SUMMARY_BACKPRESSURE_BYTES) return 0;
  const t0 = nowMs();
  while (
    pendingSummaryBytes > SUMMARY_BACKPRESSURE_BYTES &&
    pendingSummaryBulks.length > 0
  ) {
    await timer.timeAsync('idb_writes', () =>
      Promise.race(pendingSummaryBulks).then(() => undefined),
    );
  }
  return nowMs() - t0;
}

function dispatchAccum(decoderIndex: number) {
  const acc = decoderAccums[decoderIndex];
  if (acc.chunks.length === 0) return;
  const sp = spanRecorder.begin('dispatch_batch');

  const batchId = nextBatchId++;
  sp.set('decoder', decoderIndex);
  sp.set('batchId', batchId);
  sp.set('chunkCount', acc.chunks.length);
  sp.set('msgCount', acc.totalMessages);
  sp.set('totalBytes', acc.totalBytes);

  const payload: DecodeBatchRequest = {
    type: 'decode',
    batchId,
    startEventIndex: acc.startEventIndex,
    hasInvocationFirst: acc.hasInvocationFirst,
    chunks: acc.chunks,
  };

  outstandingDecoderBatches++;
  // Transfer every chunk buffer in the batch — no manual pack, no
  // memcpy. The orchestrator re-emits the transfer list as-is.
  post({
    type: 'toDecoder',
    decoder: decoderIndex,
    payload,
    transfer: acc.chunks.map(c => c.buffer),
  });

  // Reset the accumulator for the next batch destined to this decoder.
  decoderAccums[decoderIndex] = freshAccum(
    acc.startEventIndex + acc.totalMessages,
  );
  sp.end();
}

function rotateDecoder() {
  nextDecoder = (nextDecoder + 1) % initParams!.numDecoders;
}

async function ingestChunk(buf: ArrayBuffer) {
  const sp = spanRecorder.begin('dispatch_chunk');
  const chunkU8 = new Uint8Array(buf);
  decompressedBytes += chunkU8.length;
  sp.set('chunkBytes', chunkU8.length);

  // If the IDB write queue is over its memory cap, await drain. In
  // steady state this is a no-op; only fires when writes are falling
  // behind input. Recorded as a span attr so a slow dispatch_chunk in
  // the timeline reveals "X ms of that was waiting on IDB writes."
  const chunkBpMs = await awaitChunkBackpressure();
  if (chunkBpMs > 0) sp.set('chunkBackpressureWaitMs', Math.round(chunkBpMs));
  const summaryBpMs = await awaitSummaryBackpressure();
  if (summaryBpMs > 0)
    sp.set('summaryBackpressureWaitMs', Math.round(summaryBpMs));

  // Parse offsets in place (no per-message memcpy). `leading` is the
  // reassembled straddler from the previous chunk, if any.
  const parseT0 = nowMs();
  const {leading, offsets, lengths} = parser.parseInPlace(chunkU8);
  const parseMs = nowMs() - parseT0;
  timer.add('varint_parse', parseMs);
  if (parseMs > 1) sp.set('parseMs', Math.round(parseMs * 100) / 100);

  const inChunkCount = offsets.length;
  const leadingCount = leading ? 1 : 0;
  const msgCount = inChunkCount + leadingCount;
  sp.set('msgCount', msgCount);
  if (leading) sp.set('leadingBytes', leading.length);
  if (msgCount === 0) {
    sp.end();
    return;
  }

  // Assign IDB chunk indices and queue writes. Leading (if any) gets
  // its own tiny row; the in-chunk row is the decompress chunk verbatim.
  // Synchronous (fire-and-forget bulkPut at the buffer threshold) — the
  // backpressure check above is the only thing that can stall.
  const bulksBefore = pendingChunkBulks.length;
  let leadingIdbIdx: number | undefined;
  if (leading) {
    leadingIdbIdx = nextIdbChunkIndex++;
    enqueueChunkWrite(leadingIdbIdx, leading.buffer as ArrayBuffer);
  }
  let inChunkIdbIdx = 0; // unused if inChunkCount === 0
  if (inChunkCount > 0) {
    inChunkIdbIdx = nextIdbChunkIndex++;
    enqueueChunkWrite(inChunkIdbIdx, buf);
  }
  if (pendingChunkBulks.length > bulksBefore) {
    sp.set('triggeredBulkPut', true);
  }
  sp.set('pendingChunkBytes', pendingChunkBytes);
  sp.set('pendingChunkBulks', pendingChunkBulks.length);
  sp.set('pendingSummaryBytes', pendingSummaryBytes);
  if (lastChunkBulkMs > 0) {
    sp.set('lastChunkBulkMs', Math.round(lastChunkBulkMs));
    sp.set('lastChunkBulkRows', lastChunkBulkRows);
  }
  if (lastSummaryBulkMs > 0) {
    sp.set('lastSummaryBulkMs', Math.round(lastSummaryBulkMs));
    sp.set('lastSummaryBulkRows', lastSummaryBulkRows);
  }

  // A single decoder batch must fit through `summariesToBatch`, which
  // caps at SUMMARY_BATCH_SIZE. Adding this chunk to the current accum
  // would overflow → dispatch the existing accum first so this chunk
  // starts a fresh batch.
  if (msgCount > SUMMARY_BATCH_SIZE) {
    // Single chunk is bigger than the per-batch cap. We can't split it
    // (that would require sharing the chunk buffer across decoders,
    // breaking the transfer-not-clone story). Fail loudly so the
    // mismatch is visible — in practice fzstd output chunks are well
    // under this size.
    throw new Error(
      `dispatch-worker: chunk has ${msgCount} messages > SUMMARY_BATCH_SIZE (${SUMMARY_BATCH_SIZE})`,
    );
  }
  let acc = decoderAccums[nextDecoder];
  if (
    acc.totalMessages > 0 &&
    acc.totalMessages + msgCount > SUMMARY_BATCH_SIZE
  ) {
    dispatchAccum(nextDecoder);
    rotateDecoder();
    acc = decoderAccums[nextDecoder];
  }

  // Append a ChunkDescriptor to the (now safely-sized) decoder accum.
  const desc: ChunkDescriptor = {
    buffer: buf,
    offsets,
    lengths,
    idbChunkIndex: inChunkIdbIdx,
    ...(leading ? {leading, leadingIdbChunkIndex: leadingIdbIdx} : {}),
  };
  acc.chunks.push(desc);
  acc.totalBytes += chunkU8.length + (leading?.length ?? 0);
  acc.totalMessages += msgCount;

  totalEvents += msgCount;

  if (
    acc.totalBytes >= DECODER_BATCH_TARGET_BYTES ||
    acc.totalMessages >= DECODER_BATCH_TARGET_MESSAGES
  ) {
    dispatchAccum(nextDecoder);
    rotateDecoder();
  }
  sp.end();
}

function mergePartials(): AggregateData {
  // CriticalPath: pick the partial with the lowest event index for the match.
  let critPath: AggregateData['criticalPath'] = null;
  let critPathIdx = Number.POSITIVE_INFINITY;
  for (const p of partials) {
    if (p.criticalPath && p.criticalPathEventIndex < critPathIdx) {
      critPath = p.criticalPath;
      critPathIdx = p.criticalPathEventIndex;
    }
  }

  // Concat span lists in source order (helpers don't have a global order, but
  // the lists are unordered semantically — they're aggregates).
  const loadSpans = partials.flatMap(p => p.loadSpans);
  const actionSpans = partials.flatMap(p => p.actionSpans);
  const analysisSpans = partials.flatMap(p => p.analysisSpans);

  // Run LoadPackageCollector.finalize() across the merged set, using the
  // union of helper-side buildFileMetrics maps. We re-create a stub
  // collector to drive the existing finalize logic.
  const merged = new LoadPackageCollector();
  for (const s of loadSpans) merged.results.push(s);
  const mergedMap = (
    merged as unknown as {
      buildFileMetrics: Map<
        string,
        {
          starlarkPeakAllocatedBytes?: number;
          cpuInstructionCount?: number;
          targetCount?: number;
        }
      >;
    }
  ).buildFileMetrics;
  for (const p of partials) {
    for (const [k, v] of p.loadBuildFileMetrics) mergedMap.set(k, v);
  }
  merged.finalize();

  return {
    loadSpans: merged.results,
    actionSpans,
    analysisSpans,
    criticalPath: critPath,
  };
}

// Serialize message handlers — chunks AND decompressDone do `await` work
// against shared state (chunkParts, chunkSize, chunkIndex, accumulators).
// Without serialization the next `onmessage` fires while the previous is
// awaiting, interleaving mutations and corrupting state.
//
// `decoderResult` is intentionally NOT routed through this queue: it
// doesn't mutate the chunk-ingest state, and decompressDone may sit in
// the queue waiting on the LAST decoderResult to arrive — if that
// decoderResult is queued behind decompressDone we deadlock.
let processingTail: Promise<void> = Promise.resolve();

self.onmessage = (e: MessageEvent<DispatchRequest>) => {
  if (e.data.type === 'ping') {
    // Clock-sync handshake. Stamp epoch and reply immediately with a
    // `handshake` span so main's timeline has a visible sync point.
    const tEpoch = nowMs();
    spanRecorder.spans.push({
      phase: 'handshake',
      startMs: tEpoch,
      endMs: nowMs(),
    });
    post({type: 'pong', epochMs: tEpoch, spans: spanRecorder.drain()});
    return;
  }
  if (e.data.type === 'decoderResult') {
    handleDecoderResult(e.data);
    return;
  }
  processingTail = processingTail.then(() => handleMessage(e.data));
};

/** Handles a decoder result synchronously (no await). Doesn't touch
 *  chunk-ingest state, so safe to interleave with anything else. */
function handleDecoderResult(
  msg: Extract<DispatchRequest, {type: 'decoderResult'}>,
): void {
  outstandingDecoderBatches--;
  partials.push(msg.partial);
  // Note: decoder spans/epoch are accumulated on the main thread by the
  // orchestrator, which routes every `result` through itself.

  // Buffer + emit summaryBatches in batchId order.
  completedBatches.set(msg.batchId, msg.batchData);
  while (completedBatches.has(nextEmitBatchId)) {
    const bd = completedBatches.get(nextEmitBatchId)!;
    completedBatches.delete(nextEmitBatchId);
    post({type: 'summaryBatch', batch: bd});
    nextEmitBatchId++;
  }

  // Buffer for the next bulkPut. Fire-and-forget — the drain step at
  // decompressDone awaits all in-flight summary bulks. Synchronous so it
  // can't queue this handler behind anything else.
  enqueueSummaryWrite(msg.batchId, msg.batchData);

  // If decompression has already finished and we just hit zero
  // outstanding batches, signal the decompressDone handler to proceed.
  if (decompressionDone && outstandingDecoderBatches === 0 && resolveAllDone) {
    resolveAllDone();
    resolveAllDone = null;
  }
}

async function handleMessage(msg: DispatchRequest): Promise<void> {
  if (msg.type === 'init') {
    initParams = {
      numDecoders: msg.numDecoders,
      eventLogPath: msg.eventLogPath,
      compressedSize: msg.compressedSize,
    };
    decoderAccums = Array.from({length: msg.numDecoders}, () => freshAccum(0));
    try {
      await timer.timeAsync('idb_writes', () =>
        startCachingInvocation(msg.eventLogPath, msg.compressedSize),
      );
    } catch (err) {
      post({
        type: 'error',
        message: err instanceof Error ? err.message : 'init failed',
      });
    }
    return;
  }

  if (msg.type === 'chunk') {
    try {
      await ingestChunk(msg.data);
      post({
        type: 'progress',
        message: `Decoded ${totalEvents} events (${(decompressedBytes / (1024 * 1024)).toFixed(0)}MB decompressed)…`,
        decompressedBytes,
        phaseTimings: timer.report(),
      });
    } catch (err) {
      post({
        type: 'error',
        message: err instanceof Error ? err.message : 'chunk failed',
      });
    }
    return;
  }

  if (msg.type === 'decompressDone') {
    // Flush any in-flight per-decoder batches.
    for (let d = 0; d < initParams!.numDecoders; d++) {
      if (decoderAccums[d].chunks.length > 0) dispatchAccum(d);
    }

    decompressionDone = true;
    decompressedBytes = msg.decompressedBytes;
    receivedBytes = msg.receivedBytes;

    // Wait for all decoder results to come back.
    if (outstandingDecoderBatches > 0) {
      await spanRecorder.timeAsync(
        'await_last_decoders',
        () =>
          new Promise<void>(res => {
            resolveAllDone = res;
          }),
      );
    }

    // Merge partials → produces the aggregates payload main needs.
    const aggregates = spanRecorder.time('merge_partials', () =>
      mergePartials(),
    );

    // For large logs, write the heavy aggregates to IDB *before* posting
    // `done` so main can read them via the lazy-aggregate path as soon
    // as it transitions to loaded. (We can't postMessage them at this
    // scale — structured clone OOMs.)
    const inline = receivedBytes < INLINE_AGGREGATES_COMPRESSED_BYTES;
    if (!inline) {
      try {
        await spanRecorder.timeAsync('write_lazy_aggregates', () =>
          timer.timeAsync('idb_writes', () =>
            writeLazyAggregates(initParams!.eventLogPath, aggregates),
          ),
        );
      } catch (err) {
        post({
          type: 'error',
          message:
            err instanceof Error ? err.message : 'writeLazyAggregates failed',
        });
        return;
      }
    }

    // Post `done` immediately so main can transition to `loaded`. The
    // remaining tail work (drain pending writes + finalizeInvocation) is
    // pure cache-durability and runs in the background.
    post({
      type: 'done',
      decompressedBytes,
      receivedBytes,
      totalEvents,
      chunkCount: nextIdbChunkIndex,
      criticalPath: aggregates.criticalPath,
      inlineAggregates: inline
        ? {
            loadSpans: aggregates.loadSpans,
            analysisSpans: aggregates.analysisSpans,
            actionSpans: aggregates.actionSpans,
          }
        : undefined,
      phaseTimings: timer.report(),
      spans: spanRecorder.drain(),
    });

    // ---- Background cache-write tail ----
    // We don't await this from the message handler so the dispatcher's
    // processing queue closes promptly; the worker stays alive on the
    // pending Promises until the orchestrator terminates it on
    // `cacheWritten`.
    void (async () => {
      try {
        // Flush any partial buffers (both are sync — they just queue
        // bulkPuts without awaiting), then await every in-flight bulkPut.
        flushChunkBuffer();
        flushSummaryBuffer();
        if (pendingChunkBulks.length > 0 || pendingSummaryBulks.length > 0) {
          await spanRecorder.timeAsync('drain_pending_writes', () =>
            timer.timeAsync('idb_writes', async () => {
              await Promise.all(pendingChunkBulks);
              await Promise.all(pendingSummaryBulks);
            }),
          );
          pendingChunkBulks = [];
          pendingSummaryBulks = [];
        }

        await spanRecorder.timeAsync('finalize_invocation', () =>
          timer.timeAsync('idb_writes', async () => {
            // Small logs: lazy aggregates haven't been written yet
            // (they were sent inline); write them now in the background.
            if (inline) {
              await writeLazyAggregates(initParams!.eventLogPath, aggregates);
            }
            await finalizeInvocation(
              initParams!.eventLogPath,
              aggregates.criticalPath,
              nextIdbChunkIndex,
              nextBatchId,
              totalEvents,
              receivedBytes,
              decompressedBytes,
            );
          }),
        );
      } catch (err) {
        post({
          type: 'error',
          message: err instanceof Error ? err.message : 'cache write failed',
        });
        return;
      }

      post({
        type: 'cacheWritten',
        tailSpans: spanRecorder.drain(),
        tailPhaseTimings: timer.report(),
      });
    })();
    return;
  }

  // decoderResult is handled outside the serialization queue — see
  // handleDecoderResult above.
}
