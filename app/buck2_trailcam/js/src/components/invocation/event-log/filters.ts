/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import type {EventSummary} from '../../../lib/event-log-decoder';

// --- Filter types ---

export type ActiveFilter = FieldFilter | SpanFilter;

export interface FieldFilter {
  kind: 'field';
  id: string;
  path: string;
  operator: 'eq' | 'neq' | 'contains';
  value: string;
  label: string;
}

export interface SpanFilter {
  kind: 'span';
  id: string;
  spanId: number;
  label: string;
  /** Precomputed set of all transitive descendant span IDs */
  descendants: Set<number>;
}

// --- Utilities ---

let filterId = 0;
export function nextFilterId(): string {
  return `f${++filterId}`;
}

/**
 * Traverse a dot-notation path into a nested object.
 * e.g. getNestedValue(obj, "spanEnd.actionExecution.executionKind")
 */
export function getNestedValue(obj: unknown, path: string): unknown {
  const parts = path.split('.');
  let current: unknown = obj;
  for (const part of parts) {
    if (current == null || typeof current !== 'object') return undefined;
    current = (current as Record<string, unknown>)[part];
  }
  return current;
}

/**
 * Evaluate all active filters against a single event.
 * Filters are AND-combined: all must pass.
 *
 * Summary-level filters (span, type, eventType) are fast.
 * Field filters on data.* paths decode the full event on demand (slow path).
 */
export function evaluateFilters(
  event: EventSummary,
  filters: ActiveFilter[],
  getEventData?: (s: EventSummary) => Record<string, unknown>,
): boolean {
  for (const filter of filters) {
    if (filter.kind === 'span') {
      if (event.spanId == null) return false;
      if (
        event.spanId !== filter.spanId &&
        !filter.descendants.has(event.spanId)
      ) {
        return false;
      }
    } else {
      let val: unknown;
      if (filter.path === 'type') {
        val = event.type;
      } else if (filter.path === 'eventType') {
        val = event.eventType;
      } else if (getEventData) {
        // Slow path: decode full event data for field filters
        val = getNestedValue(getEventData(event), filter.path);
      }
      const strVal = String(val ?? '');
      switch (filter.operator) {
        case 'eq':
          if (strVal !== filter.value) return false;
          break;
        case 'neq':
          if (strVal === filter.value) return false;
          break;
        case 'contains':
          if (!strVal.toLowerCase().includes(filter.value.toLowerCase()))
            return false;
          break;
      }
    }
  }
  return true;
}

/**
 * Check if a specific path+value is currently filtered on.
 */
export function hasActiveFieldFilter(
  filters: ActiveFilter[],
  path: string,
  value: string,
): boolean {
  return filters.some(
    f => f.kind === 'field' && f.path === path && f.value === value,
  );
}

// --- URL serialization ---
//
// Field filters: `field:<path>:<op>:<value>` (value may contain ':')
// Span filters:  `span:<spanId>` (descendants are recomputed on hydration)

function makeFieldLabel(
  path: string,
  operator: FieldFilter['operator'],
  value: string,
): string {
  const shortPath = path.split('.').pop() ?? path;
  const opSym = operator === 'neq' ? '≠' : operator === 'contains' ? '~' : '=';
  return `${shortPath} ${opSym} ${value}`;
}

export function serializeFilter(f: ActiveFilter): string {
  if (f.kind === 'span') return `span:${f.spanId}`;
  return `field:${f.path}:${f.operator}:${f.value}`;
}

/**
 * Decode a filter string. For span filters, descendants are computed
 * via `resolveDescendants(spanId)`.
 */
export function parseFilter(
  s: string,
  resolveDescendants: (spanId: number) => Set<number>,
): ActiveFilter | null {
  if (s.startsWith('span:')) {
    const id = parseInt(s.slice(5), 10);
    if (!Number.isFinite(id)) return null;
    return {
      kind: 'span',
      id: nextFilterId(),
      spanId: id,
      label: `span ${id}`,
      descendants: resolveDescendants(id),
    };
  }
  if (s.startsWith('field:')) {
    const rest = s.slice('field:'.length);
    const i1 = rest.indexOf(':');
    if (i1 < 0) return null;
    const i2 = rest.indexOf(':', i1 + 1);
    if (i2 < 0) return null;
    const path = rest.slice(0, i1);
    const op = rest.slice(i1 + 1, i2) as FieldFilter['operator'];
    if (op !== 'eq' && op !== 'neq' && op !== 'contains') return null;
    const value = rest.slice(i2 + 1);
    return {
      kind: 'field',
      id: nextFilterId(),
      path,
      operator: op,
      value,
      label: makeFieldLabel(path, op, value),
    };
  }
  return null;
}
