/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import type {
  EventSummaryStore,
  SummaryView,
} from '../../../lib/event-summary-store';

/**
 * Compact span hierarchy index.
 *
 * Memory-conscious design:
 *   - childrenOf: parent-spanId → child-spanId mapping in a compressed
 *     sparse row (CSR) layout. For ~1M parents averaging ~2 children
 *     this is ~28MB total — vs ~200MB if we used Map<number, Set<number>>
 *     (every Set carries a ~190-byte hash-table FixedArray plus a Set
 *     header, even for entries with 1-3 children).
 *   - firstEventOf: spanId → index of the first event for that span.
 *     Used to derive parentOf and spanType lazily from the underlying
 *     store.
 *
 * Things deliberately omitted (vs the old design):
 *   - parentOf — derived via store.getParentId(firstEventOf.get(spanId))
 *   - spanType — derived via store.getEventType(firstEventOf.get(spanId))
 *   - eventsOf (full event list per span) — only firstEventOf is ever read
 *     by callers (findSpanStartEvent only returns events[0]); span filtering
 *     uses descendants + filter on summary.spanId, not eventsOf
 */

/**
 * CSR view of the parent → children map. Public surface is a single
 * `get(parentId)` lookup that returns a Float64Array view of the
 * parent's child spanIds, or undefined if the parent has no children.
 */
export class ChildrenIndex {
  /** Sorted distinct parent spanIds (one entry per CSR row). */
  private readonly parentIds: Float64Array;
  /** Offsets into `childrenList`. Length = parentIds.length + 1.
   *  Row i's children occupy `childrenList[childrenStart[i] .. childrenStart[i+1])`. */
  private readonly childrenStart: Uint32Array;
  /** Flat list of child spanIds, grouped per parent in `parentIds` order. */
  private readonly childrenList: Float64Array;

  constructor(
    parentIds: Float64Array,
    childrenStart: Uint32Array,
    childrenList: Float64Array,
  ) {
    this.parentIds = parentIds;
    this.childrenStart = childrenStart;
    this.childrenList = childrenList;
  }

  /** Number of distinct parents tracked (for diagnostics). */
  get parentCount(): number {
    return this.parentIds.length;
  }

  /** Number of (parent, child) edges (for diagnostics). */
  get edgeCount(): number {
    return this.childrenList.length;
  }

  /**
   * Look up a parent's children. Returns a typed-array view (no copy)
   * into the shared `childrenList`, so iterate it before any mutation
   * (we don't mutate post-construction).
   */
  get(parentId: number): Float64Array | undefined {
    const idx = this.findParent(parentId);
    if (idx < 0) return undefined;
    const start = this.childrenStart[idx];
    const end = this.childrenStart[idx + 1];
    return start < end ? this.childrenList.subarray(start, end) : undefined;
  }

  private findParent(parentId: number): number {
    const arr = this.parentIds;
    let lo = 0;
    let hi = arr.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      const v = arr[mid];
      if (v < parentId) lo = mid + 1;
      else if (v > parentId) hi = mid;
      else return mid;
    }
    return -1;
  }
}

export interface SpanIndex {
  /** parent spanId → child spanIds (CSR-backed). */
  childrenOf: ChildrenIndex;
  /** spanId → first event store index for that span */
  firstEventOf: Map<number, number>;
}

/**
 * Build a span hierarchy index from a columnar event summary store.
 * Iterates the store with direct accessors (no EventSummary materialization).
 *
 * Build proceeds in two phases:
 *   1. Single pass over events to populate `firstEventOf` and a transient
 *      `Map<parentId, number[]>` of children-in-encounter-order.
 *   2. Convert that map to the CSR layout: sort parentIds, prefix-sum
 *      to childrenStart, copy children into childrenList.
 *
 * The transient map is the dominant build-time memory cost (~80-100MB
 * peak for a 1M-parent log) but is GC'd once CSR is built. Steady-state
 * is ~28MB for the CSR plus the firstEventOf Map.
 */
export function buildSpanIndex(events: EventSummaryStore): SpanIndex {
  const firstEventOf = new Map<number, number>();
  const childMap = new Map<number, number[]>();

  for (let i = 0; i < events.length; i++) {
    const spanId = events.getSpanId(i);
    if (spanId == null) continue;

    if (!firstEventOf.has(spanId)) {
      firstEventOf.set(spanId, i);

      // Track parent-child relationships only on the first occurrence
      // (consistent with the old !parentOf.has(spanId) gate). Each
      // spanId is added at most once to its parent's child list, so
      // no dedup is needed here even though we replaced Set with Array.
      const parentId = events.getParentId(i);
      if (parentId != null) {
        let kids = childMap.get(parentId);
        if (!kids) {
          kids = [];
          childMap.set(parentId, kids);
        }
        kids.push(spanId);
      }
    }
  }

  // ---- Convert childMap → CSR ----
  const numParents = childMap.size;
  let totalEdges = 0;
  for (const kids of childMap.values()) totalEdges += kids.length;

  const parentIds = new Float64Array(numParents);
  const childrenStart = new Uint32Array(numParents + 1);
  const childrenList = new Float64Array(totalEdges);

  // Sort parents so `get()` can binary-search.
  const sortedParents: number[] = Array.from(childMap.keys()).sort(
    (a, b) => a - b,
  );

  let pos = 0;
  for (let i = 0; i < sortedParents.length; i++) {
    const pid = sortedParents[i];
    parentIds[i] = pid;
    childrenStart[i] = pos;
    const kids = childMap.get(pid)!;
    for (let j = 0; j < kids.length; j++) {
      childrenList[pos++] = kids[j];
    }
  }
  childrenStart[numParents] = pos;

  return {
    childrenOf: new ChildrenIndex(parentIds, childrenStart, childrenList),
    firstEventOf,
  };
}

/**
 * Get the eventType for a span (e.g., "analysis", "actionExecution").
 * Derived from the span's first event in the store.
 */
export function getSpanType(
  spanIndex: SpanIndex,
  store: SummaryView,
  spanId: number,
): string | undefined {
  const idx = spanIndex.firstEventOf.get(spanId);
  if (idx == null) return undefined;
  return store.getEventType(idx);
}

/**
 * Get all transitive descendant span IDs of a given span.
 * Uses BFS to avoid stack overflow on deep hierarchies.
 */
export function getDescendants(
  spanIndex: SpanIndex,
  rootSpanId: number,
): Set<number> {
  const descendants = new Set<number>();
  const queue = [rootSpanId];

  while (queue.length > 0) {
    const current = queue.pop()!;
    const children = spanIndex.childrenOf.get(current);
    if (children) {
      for (let i = 0; i < children.length; i++) {
        const child = children[i];
        if (!descendants.has(child)) {
          descendants.add(child);
          queue.push(child);
        }
      }
    }
  }

  return descendants;
}

/**
 * Get the ancestry chain (from root to the given span).
 * Walks up by reading parentId from each ancestor's first event in the store.
 */
export function getSpanAncestry(
  spanIndex: SpanIndex,
  store: SummaryView,
  spanId: number,
): {spanId: number; eventType: string}[] {
  const chain: {spanId: number; eventType: string}[] = [];
  let current: number | undefined = spanId;

  while (current != null) {
    const firstIdx = spanIndex.firstEventOf.get(current);
    chain.push({
      spanId: current,
      eventType:
        (firstIdx != null ? store.getEventType(firstIdx) : undefined) ??
        'unknown',
    });
    if (firstIdx == null) break;
    const parent = store.getParentId(firstIdx);
    if (parent == null) break;
    current = parent;
  }

  chain.reverse();
  return chain;
}

/**
 * Find the first event index for a given spanId (the span start).
 */
export function findSpanStartEvent(
  spanIndex: SpanIndex,
  spanId: number,
): number | undefined {
  return spanIndex.firstEventOf.get(spanId);
}
