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
 * Columnar storage for event summaries — uses parallel typed arrays + string
 * pools instead of JS objects per event.
 *
 * Internal structure: data is split into fixed-size batches (BATCH_SIZE
 * events each, except the last which may be shorter). Each batch is a
 * self-contained columnar block with its own typed arrays and string pools.
 *
 * Why batched:
 *   - IDB writes/reads happen one batch at a time (no large allocations)
 *   - Worker can post small batch payloads (small structured-clone cost)
 *   - No giant typed arrays to allocate; each batch is small (~480KB columnar)
 *   - Random access: O(1) — batchIdx = i / BATCH_SIZE, offset = i % BATCH_SIZE
 *
 * Public API mirrors EventSummary[] where reasonable:
 *   - store.length
 *   - store.get(i)              materializes an EventSummary object
 *   - store.getType(i), etc.    direct field access (no allocation)
 *
 * Filtered/sorted views use Int32Array of GLOBAL indices into the store
 * (0 to store.length - 1). The store's accessors handle batch lookup
 * internally, so FilteredView code is unchanged.
 */

import type {EventSummary} from './event-log-decoder';

/** Events per batch. All batches except the last are exactly this size. */
export const SUMMARY_BATCH_SIZE = 10000;

const UNDEFINED_POOL_IDX = 0;

/**
 * Intern a string into the pool. The pool's values[0] is reserved for
 * undefined/empty. The lookup map is used during construction; once the
 * batch is sealed, only the values array is needed (read-only access).
 */
function intern(
  values: string[],
  index: Map<string, number>,
  s: string | undefined,
): number {
  if (s == null || s === '') return UNDEFINED_POOL_IDX;
  let idx = index.get(s);
  if (idx == null) {
    idx = values.length;
    values.push(s);
    index.set(s, idx);
  }
  return idx;
}

/**
 * One columnar batch. All typed arrays have the same length (`length` field).
 */
export interface BatchData {
  length: number;
  // Numeric columns
  indexCol: Int32Array;
  timestampMsCol: Float64Array; // NaN = undefined
  spanIdCol: Float64Array; // 0 = undefined
  parentIdCol: Float64Array; // 0 = undefined
  durationMsCol: Float64Array; // NaN = undefined
  batchIndexCol: Uint32Array; // proto chunk index, not summary batch
  offsetInBatchCol: Uint32Array;
  lengthInBatchCol: Uint32Array;
  // Pool indices (0 = undefined sentinel)
  typeIdxCol: Uint8Array;
  eventTypeIdxCol: Uint16Array;
  actionNameIdxCol: Uint16Array;
  executionKindIdxCol: Uint8Array;
  targetLabelIdxCol: Uint32Array;
  // Failed flag for action-execution spanEnd events:
  //   0 = success, 1 = failed, 255 = not applicable (any non-action-spanEnd row)
  failedCol: Uint8Array;
  // Per-batch string pools (small — duplication across batches is negligible)
  typePool: string[];
  eventTypePool: string[];
  actionNamePool: string[];
  executionKindPool: string[];
  targetLabelPool: string[];
}

/**
 * Read-only interface implemented by both EventSummaryStore and FilteredView.
 */
export interface SummaryView {
  readonly length: number;
  get(i: number): EventSummary;
  getIndex(i: number): number;
  getType(i: number): string;
  getEventType(i: number): string | undefined;
  getTimestampMs(i: number): number | undefined;
  getSpanId(i: number): number | undefined;
  getParentId(i: number): number | undefined;
  getDurationMs(i: number): number | undefined;
  getActionName(i: number): string | undefined;
  getExecutionKind(i: number): string | undefined;
  getTargetLabel(i: number): string | undefined;
  getFailed(i: number): boolean | undefined;
  getBatchIndex(i: number): number;
  getOffsetInBatch(i: number): number;
  getLengthInBatch(i: number): number;
}

/**
 * Filtered/sorted view over a backing EventSummaryStore. Stores only an
 * Int32Array of GLOBAL indices into the store — no event data is copied.
 */
export class FilteredView implements SummaryView {
  constructor(
    private readonly store: EventSummaryStore,
    private readonly indices: Int32Array,
  ) {}

  get length(): number {
    return this.indices.length;
  }

  get(i: number): EventSummary {
    return this.store.get(this.indices[i]);
  }
  getIndex(i: number): number {
    return this.store.getIndex(this.indices[i]);
  }
  getType(i: number): string {
    return this.store.getType(this.indices[i]);
  }
  getEventType(i: number): string | undefined {
    return this.store.getEventType(this.indices[i]);
  }
  getTimestampMs(i: number): number | undefined {
    return this.store.getTimestampMs(this.indices[i]);
  }
  getSpanId(i: number): number | undefined {
    return this.store.getSpanId(this.indices[i]);
  }
  getParentId(i: number): number | undefined {
    return this.store.getParentId(this.indices[i]);
  }
  getDurationMs(i: number): number | undefined {
    return this.store.getDurationMs(this.indices[i]);
  }
  getActionName(i: number): string | undefined {
    return this.store.getActionName(this.indices[i]);
  }
  getExecutionKind(i: number): string | undefined {
    return this.store.getExecutionKind(this.indices[i]);
  }
  getTargetLabel(i: number): string | undefined {
    return this.store.getTargetLabel(this.indices[i]);
  }
  getFailed(i: number): boolean | undefined {
    return this.store.getFailed(this.indices[i]);
  }
  getBatchIndex(i: number): number {
    return this.store.getBatchIndex(this.indices[i]);
  }
  getOffsetInBatch(i: number): number {
    return this.store.getOffsetInBatch(this.indices[i]);
  }
  getLengthInBatch(i: number): number {
    return this.store.getLengthInBatch(this.indices[i]);
  }

  getStoreIndex(i: number): number {
    return this.indices[i];
  }
}

// ============================================================================
// Batch builder (used while accumulating a batch before sealing)
// ============================================================================

/**
 * Same shape as BatchData but with mutable pools and growing typed arrays
 * during construction. Pool indices are shared lookup Maps that aren't
 * needed after sealing.
 */
interface BatchBuilder extends BatchData {
  // Lookup maps used only during construction (dropped on seal)
  typePoolIndex: Map<string, number>;
  eventTypePoolIndex: Map<string, number>;
  actionNamePoolIndex: Map<string, number>;
  executionKindPoolIndex: Map<string, number>;
  targetLabelPoolIndex: Map<string, number>;
}

function newBatchBuilder(): BatchBuilder {
  return {
    length: 0,
    indexCol: new Int32Array(SUMMARY_BATCH_SIZE),
    timestampMsCol: new Float64Array(SUMMARY_BATCH_SIZE).fill(NaN),
    spanIdCol: new Float64Array(SUMMARY_BATCH_SIZE),
    parentIdCol: new Float64Array(SUMMARY_BATCH_SIZE),
    durationMsCol: new Float64Array(SUMMARY_BATCH_SIZE).fill(NaN),
    batchIndexCol: new Uint32Array(SUMMARY_BATCH_SIZE),
    offsetInBatchCol: new Uint32Array(SUMMARY_BATCH_SIZE),
    lengthInBatchCol: new Uint32Array(SUMMARY_BATCH_SIZE),
    typeIdxCol: new Uint8Array(SUMMARY_BATCH_SIZE),
    eventTypeIdxCol: new Uint16Array(SUMMARY_BATCH_SIZE),
    actionNameIdxCol: new Uint16Array(SUMMARY_BATCH_SIZE),
    executionKindIdxCol: new Uint8Array(SUMMARY_BATCH_SIZE),
    targetLabelIdxCol: new Uint32Array(SUMMARY_BATCH_SIZE),
    failedCol: new Uint8Array(SUMMARY_BATCH_SIZE).fill(255),
    typePool: [''],
    eventTypePool: [''],
    actionNamePool: [''],
    executionKindPool: [''],
    targetLabelPool: [''],
    typePoolIndex: new Map([['', 0]]),
    eventTypePoolIndex: new Map([['', 0]]),
    actionNamePoolIndex: new Map([['', 0]]),
    executionKindPoolIndex: new Map([['', 0]]),
    targetLabelPoolIndex: new Map([['', 0]]),
  };
}

/**
 * Seal a builder into an immutable BatchData. If the builder is partially
 * full (last batch in the store), the typed arrays are sliced to fit.
 * If full, references the builder's arrays directly (no copy).
 */
function sealBatch(b: BatchBuilder): BatchData {
  const len = b.length;
  const trimI32 = (a: Int32Array) => (len === a.length ? a : a.slice(0, len));
  const trimU32 = (a: Uint32Array) => (len === a.length ? a : a.slice(0, len));
  const trimF64 = (a: Float64Array) => (len === a.length ? a : a.slice(0, len));
  const trimU8 = (a: Uint8Array) => (len === a.length ? a : a.slice(0, len));
  const trimU16 = (a: Uint16Array) => (len === a.length ? a : a.slice(0, len));
  return {
    length: len,
    indexCol: trimI32(b.indexCol),
    timestampMsCol: trimF64(b.timestampMsCol),
    spanIdCol: trimF64(b.spanIdCol),
    parentIdCol: trimF64(b.parentIdCol),
    durationMsCol: trimF64(b.durationMsCol),
    batchIndexCol: trimU32(b.batchIndexCol),
    offsetInBatchCol: trimU32(b.offsetInBatchCol),
    lengthInBatchCol: trimU32(b.lengthInBatchCol),
    typeIdxCol: trimU8(b.typeIdxCol),
    eventTypeIdxCol: trimU16(b.eventTypeIdxCol),
    actionNameIdxCol: trimU16(b.actionNameIdxCol),
    executionKindIdxCol: trimU8(b.executionKindIdxCol),
    targetLabelIdxCol: trimU32(b.targetLabelIdxCol),
    failedCol: trimU8(b.failedCol),
    typePool: b.typePool,
    eventTypePool: b.eventTypePool,
    actionNamePool: b.actionNamePool,
    executionKindPool: b.executionKindPool,
    targetLabelPool: b.targetLabelPool,
  };
}

// ============================================================================
// EventSummaryStore — batched columnar storage
// ============================================================================

export class EventSummaryStore implements SummaryView {
  /** Sealed (immutable) batches. Sizes may vary — the parallel
   *  pipeline's per-decompress-chunk dispatch produces decoder
   *  batches whose final summary batch size depends on chunk
   *  message counts. The single-worker pipeline emits fixed
   *  SUMMARY_BATCH_SIZE batches and still works fine here. */
  private batches: BatchData[] = [];
  /** Cumulative event count after each sealed batch. `batchEnds[i]`
   *  is the global index *just past* the last event in batch `i`,
   *  i.e. `sum(batches[0..=i].length)`. Used by `locate()` for
   *  O(log N) batch lookups regardless of per-batch sizing. */
  private batchEnds: number[] = [];
  /** Current open batch being filled. Null if no events pushed yet or
   *  immediately after sealing. */
  private current: BatchBuilder | null = null;
  /** Total number of events across all batches. */
  private _length = 0;

  get length(): number {
    return this._length;
  }

  /** Number of sealed batches plus current open batch (if any) */
  get batchCount(): number {
    return this.batches.length + (this.current ? 1 : 0);
  }

  // --- Internal batch lookup ---

  /** Returns the batch and per-batch offset for global index `i`.
   *  Binary-searches `batchEnds` over sealed batches; falls back to
   *  the open `current` batch if `i` lies past all sealed entries. */
  private locate(i: number): {batch: BatchData | BatchBuilder; offset: number} {
    const ends = this.batchEnds;
    let lo = 0;
    let hi = ends.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if (ends[mid] <= i) lo = mid + 1;
      else hi = mid;
    }
    if (lo < this.batches.length) {
      const start = lo === 0 ? 0 : ends[lo - 1];
      return {batch: this.batches[lo], offset: i - start};
    }
    // Past all sealed batches — must be in the current open builder.
    const sealedTotal = ends.length === 0 ? 0 : ends[ends.length - 1];
    return {batch: this.current!, offset: i - sealedTotal};
  }

  // --- Direct field accessors ---

  getIndex(i: number): number {
    const {batch, offset} = this.locate(i);
    return batch.indexCol[offset];
  }

  getType(i: number): string {
    const {batch, offset} = this.locate(i);
    return batch.typePool[batch.typeIdxCol[offset]] as string;
  }

  getEventType(i: number): string | undefined {
    const {batch, offset} = this.locate(i);
    const idx = batch.eventTypeIdxCol[offset];
    return idx === 0 ? undefined : (batch.eventTypePool[idx] as string);
  }

  getTimestampMs(i: number): number | undefined {
    const {batch, offset} = this.locate(i);
    const v = batch.timestampMsCol[offset];
    return Number.isNaN(v) ? undefined : v;
  }

  getSpanId(i: number): number | undefined {
    const {batch, offset} = this.locate(i);
    const v = batch.spanIdCol[offset];
    return v === 0 ? undefined : v;
  }

  getParentId(i: number): number | undefined {
    const {batch, offset} = this.locate(i);
    const v = batch.parentIdCol[offset];
    return v === 0 ? undefined : v;
  }

  getDurationMs(i: number): number | undefined {
    const {batch, offset} = this.locate(i);
    const v = batch.durationMsCol[offset];
    return Number.isNaN(v) ? undefined : v;
  }

  getActionName(i: number): string | undefined {
    const {batch, offset} = this.locate(i);
    const idx = batch.actionNameIdxCol[offset];
    return idx === 0 ? undefined : (batch.actionNamePool[idx] as string);
  }

  getExecutionKind(i: number): string | undefined {
    const {batch, offset} = this.locate(i);
    const idx = batch.executionKindIdxCol[offset];
    return idx === 0 ? undefined : (batch.executionKindPool[idx] as string);
  }

  getTargetLabel(i: number): string | undefined {
    const {batch, offset} = this.locate(i);
    const idx = batch.targetLabelIdxCol[offset];
    return idx === 0 ? undefined : (batch.targetLabelPool[idx] as string);
  }

  getFailed(i: number): boolean | undefined {
    const {batch, offset} = this.locate(i);
    const v = batch.failedCol[offset];
    return v === 255 ? undefined : v === 1;
  }

  getBatchIndex(i: number): number {
    const {batch, offset} = this.locate(i);
    return batch.batchIndexCol[offset];
  }

  getOffsetInBatch(i: number): number {
    const {batch, offset} = this.locate(i);
    return batch.offsetInBatchCol[offset];
  }

  getLengthInBatch(i: number): number {
    const {batch, offset} = this.locate(i);
    return batch.lengthInBatchCol[offset];
  }

  /**
   * Materialize the summary at index i as an EventSummary object.
   * Use for backward compatibility or when an object is needed.
   */
  get(i: number): EventSummary {
    const result: EventSummary = {
      index: this.getIndex(i),
      type: this.getType(i),
      batchIndex: this.getBatchIndex(i),
      offsetInBatch: this.getOffsetInBatch(i),
      lengthInBatch: this.getLengthInBatch(i),
    };
    const eventType = this.getEventType(i);
    if (eventType !== undefined) result.eventType = eventType;
    const ts = this.getTimestampMs(i);
    if (ts !== undefined) result.timestampMs = ts;
    const spanId = this.getSpanId(i);
    if (spanId !== undefined) result.spanId = spanId;
    const parentId = this.getParentId(i);
    if (parentId !== undefined) result.parentId = parentId;
    const dur = this.getDurationMs(i);
    if (dur !== undefined) result.durationMs = dur;
    const actionName = this.getActionName(i);
    if (actionName !== undefined) result.actionName = actionName;
    const execKind = this.getExecutionKind(i);
    if (execKind !== undefined) result.executionKind = execKind;
    const target = this.getTargetLabel(i);
    if (target !== undefined) result.targetLabel = target;
    const failed = this.getFailed(i);
    if (failed !== undefined) result.failed = failed;
    return result;
  }

  // --- Mutation ---

  push(s: EventSummary): void {
    if (!this.current) this.current = newBatchBuilder();
    const b = this.current;
    const o = b.length;
    b.indexCol[o] = s.index;
    b.timestampMsCol[o] = s.timestampMs ?? NaN;
    b.spanIdCol[o] = s.spanId ?? 0;
    b.parentIdCol[o] = s.parentId ?? 0;
    b.durationMsCol[o] = s.durationMs ?? NaN;
    b.batchIndexCol[o] = s.batchIndex;
    b.offsetInBatchCol[o] = s.offsetInBatch;
    b.lengthInBatchCol[o] = s.lengthInBatch;
    b.typeIdxCol[o] = intern(b.typePool, b.typePoolIndex, s.type);
    b.eventTypeIdxCol[o] = intern(
      b.eventTypePool,
      b.eventTypePoolIndex,
      s.eventType,
    );
    b.actionNameIdxCol[o] = intern(
      b.actionNamePool,
      b.actionNamePoolIndex,
      s.actionName,
    );
    b.executionKindIdxCol[o] = intern(
      b.executionKindPool,
      b.executionKindPoolIndex,
      s.executionKind,
    );
    b.targetLabelIdxCol[o] = intern(
      b.targetLabelPool,
      b.targetLabelPoolIndex,
      s.targetLabel,
    );
    b.failedCol[o] = s.failed === undefined ? 255 : s.failed ? 1 : 0;
    b.length++;
    this._length++;
    if (b.length === SUMMARY_BATCH_SIZE) this.sealCurrent();
  }

  pushBatch(arr: readonly EventSummary[]): void {
    for (let i = 0; i < arr.length; i++) this.push(arr[i]);
  }

  /**
   * Push a fully-formed BatchData (e.g., loaded from IDB or produced by
   * a parallel decoder). Batches may have any length up to
   * SUMMARY_BATCH_SIZE — `locate()` uses prefix sums, so variable-size
   * batches are supported. Seals any open `current` batch first.
   */
  pushSealedBatch(b: BatchData): void {
    if (this.current) this.sealCurrent();
    this.batches.push(b);
    this._length += b.length;
    this.batchEnds.push(this._length);
  }

  /**
   * Seal the current open batch, if any. Call this when streaming is done
   * (or before pushing a sealed batch).
   */
  private sealCurrent(): void {
    if (!this.current || this.current.length === 0) {
      this.current = null;
      return;
    }
    this.batches.push(sealBatch(this.current));
    this.batchEnds.push(this._length);
    this.current = null;
  }

  /** Trim the in-progress batch's typed arrays to actual size. */
  shrinkToFit(): void {
    this.sealCurrent();
  }

  // --- Iteration / batch access (used by IDB write) ---

  /**
   * Returns all sealed batches. Call shrinkToFit() first to ensure the
   * last partial batch is sealed.
   */
  getSealedBatches(): readonly BatchData[] {
    return this.batches;
  }
}

/**
 * Convert an EventSummary[] to a single sealed BatchData. The array must
 * have at most SUMMARY_BATCH_SIZE entries.
 */
export function summariesToBatch(arr: readonly EventSummary[]): BatchData {
  if (arr.length > SUMMARY_BATCH_SIZE) {
    throw new Error(
      `summariesToBatch: input too large (${arr.length} > ${SUMMARY_BATCH_SIZE})`,
    );
  }
  const tmp = new EventSummaryStore();
  tmp.pushBatch(arr);
  tmp.shrinkToFit();
  const batches = tmp.getSealedBatches();
  if (batches.length !== 1) {
    throw new Error(
      `summariesToBatch: expected 1 batch, got ${batches.length}`,
    );
  }
  return batches[0];
}
