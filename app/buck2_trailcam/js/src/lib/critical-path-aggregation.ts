/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import type {CriticalPathEntry, PhaseGroup} from './critical-path';

/**
 * An action_execution kind is a "cache hit" (no real work done) when it
 * represents a hit against the action cache, local action cache, or a
 * dep-file based reuse. The proto strings look like
 * `ACTION_EXECUTION_KIND_ACTION_CACHE`, `..._LOCAL_ACTION_CACHE`,
 * `..._LOCAL_DEP_FILE`.
 */
export function isCacheHitKind(kind: string | undefined): boolean {
  if (!kind) return false;
  return kind.includes('CACHE') || kind.includes('DEP_FILE');
}

/**
 * Aggregation key for the phase summary breakdown.
 * - generic_entry → `generic:<kind>` (each kind is a separate row)
 * - action_execution: cache-hit kinds collapse to `action:cached`,
 *   everything else aggregates by category as `action:<category>`
 * - waiting → `waiting` (rendered as "overhead")
 * - everything else → entry.type
 */
export function aggregationKey(entry: CriticalPathEntry): string {
  if (entry.type === 'generic_entry') {
    return `generic:${entry.label || 'unknown'}`;
  }
  if (entry.type === 'action_execution') {
    if (isCacheHitKind(entry.executionKind)) return 'action:cached';
    return `action:${entry.actionCategory || 'uncategorized'}`;
  }
  return entry.type;
}

export function formatAggregationKey(key: string): string {
  if (key === 'waiting') return 'overhead';
  if (key === 'action:cached') return 'cached actions';
  if (key.startsWith('action:'))
    return `action: ${key.slice('action:'.length)}`;
  if (key.startsWith('generic:')) {
    return key.slice('generic:'.length).replace(/[_-]/g, ' ');
  }
  return key.replace(/_/g, ' ');
}

export interface AggregatedRow {
  key: string;
  count: number;
  totalMs: number;
}

/** Aggregate a phase group's entries by `aggregationKey`, sorted by total. */
export function aggregateGroup(group: PhaseGroup): AggregatedRow[] {
  const byKey = new Map<string, AggregatedRow>();
  for (const entry of group.entries) {
    const key = aggregationKey(entry);
    const existing = byKey.get(key);
    if (existing) {
      existing.count++;
      existing.totalMs += entry.wallDurationMs;
    } else {
      byKey.set(key, {key, count: 1, totalMs: entry.wallDurationMs});
    }
  }
  return Array.from(byKey.values()).sort((a, b) => b.totalMs - a.totalMs);
}
