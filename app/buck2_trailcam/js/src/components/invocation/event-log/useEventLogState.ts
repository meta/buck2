/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {useEffect, useState, useMemo, useCallback, useRef} from 'react';
import type {EventSummary} from '../../../lib/event-log-decoder';
import {
  type ActiveFilter,
  type FieldFilter,
  evaluateFilters,
  nextFilterId,
  parseFilter,
  serializeFilter,
} from './filters';
import {buildSpanIndex, getDescendants, type SpanIndex} from './span-index';
import {
  FilteredView,
  type EventSummaryStore,
  type SummaryView,
} from '../../../lib/event-summary-store';
import {readUrlList, useUrlState, writeUrlList} from '../../../lib/url-state';

// Shared TextDecoder for searching raw proto bytes as UTF-8 strings
const textDecoder = new TextDecoder('utf-8', {fatal: false});

export type SortField =
  | 'index'
  | 'timestampMs'
  | 'type'
  | 'eventType'
  | 'durationMs'
  | 'actionName'
  | 'executionKind'
  | 'targetLabel';
export type SortDir = 'asc' | 'desc';

/** Columns that can be toggled visible/hidden */
export const TOGGLEABLE_COLUMNS = [
  {id: 'durationMs' as const, label: 'Duration'},
  {id: 'actionName' as const, label: 'Action'},
  {id: 'executionKind' as const, label: 'Exec'},
  {id: 'targetLabel' as const, label: 'Target'},
  {id: 'spanId' as const, label: 'Span'},
  {id: 'parentId' as const, label: 'Parent'},
];

const DEFAULT_HIDDEN = new Set(['spanId', 'parentId']);
const DEFAULT_HIDDEN_STR = [...DEFAULT_HIDDEN].sort().join(',');

function setToCols(s: Set<string>): string {
  return [...s].sort().join(',');
}

function colsToSet(s: string): Set<string> {
  return new Set(s.split(',').filter(Boolean));
}

function parseSort(s: string): {field: SortField | null; dir: SortDir} {
  const [field, dir] = s.split(':');
  const validFields: SortField[] = [
    'index',
    'timestampMs',
    'type',
    'eventType',
    'durationMs',
    'actionName',
    'executionKind',
    'targetLabel',
  ];
  return {
    field: validFields.includes(field as SortField)
      ? (field as SortField)
      : null,
    dir: dir === 'desc' ? 'desc' : 'asc',
  };
}

function stringifySort(field: SortField | null, dir: SortDir): string {
  return field ? `${field}:${dir}` : '';
}

function sortValueAt(
  summaries: EventSummaryStore,
  i: number,
  field: SortField,
): string | number {
  switch (field) {
    case 'index':
      return summaries.getIndex(i);
    case 'type':
      return summaries.getType(i);
    case 'eventType':
      return summaries.getEventType(i) ?? '';
    case 'timestampMs':
      return summaries.getTimestampMs(i) ?? 0;
    case 'durationMs':
      return summaries.getDurationMs(i) ?? 0;
    default:
      return '';
  }
}

export function useEventLogState(
  summaries: EventSummaryStore,
  getEventData: (s: EventSummary) => Record<string, unknown>,
  getEventBytes: (s: EventSummary) => Uint8Array,
  isLargeLog = false,
) {
  // Selection / search / view mode — straight URL-backed state.
  const [selectedIndex, setSelectedIndexRaw] = useUrlState<number | null>(
    'sel',
    null,
    {
      parse: s => {
        const n = parseInt(s, 10);
        return Number.isFinite(n) ? n : null;
      },
      stringify: v => (v == null ? '' : String(v)),
    },
  );
  const [textSearch, setTextSearch] = useUrlState<string>('q', '');
  const [inlineDetails, setInlineDetailsRaw] = useUrlState<boolean>(
    'inline',
    false,
    {
      parse: s => s === '1',
      stringify: v => (v ? '1' : '0'),
    },
  );
  const setInlineDetails = useCallback(
    (v: boolean | ((prev: boolean) => boolean)) => {
      const next = typeof v === 'function' ? v(inlineDetails) : v;
      setInlineDetailsRaw(next);
    },
    [inlineDetails, setInlineDetailsRaw],
  );

  // Sort: encoded as `field:dir` (e.g. `duration:desc`), omitted when no sort.
  const [sortRaw, setSortRaw] = useUrlState<string>('sort', '');
  const {field: sortField, dir: sortDir} = useMemo(
    () => (sortRaw ? parseSort(sortRaw) : {field: null, dir: 'asc' as SortDir}),
    [sortRaw],
  );

  // Hidden columns: comma-separated, omitted when matches the default set.
  const [colsRaw, setColsRaw] = useUrlState<string>('cols', DEFAULT_HIDDEN_STR);
  const hiddenColumns = useMemo(() => colsToSet(colsRaw), [colsRaw]);

  // Filters: hydrated from URL on mount; synced back to URL on every change.
  // Span filters need spanIndex to resolve descendants, so the index is
  // built eagerly when the URL contains a span filter.
  const [activeFilters, setActiveFilters] = useState<ActiveFilter[]>([]);

  // Lazy span index — built only on first access (most browsing doesn't
  // touch span features). The index can use ~hundreds of MB for very large
  // logs, so we don't pay that cost unless the user selects an event or
  // adds a span filter.
  const spanIndexCacheRef = useRef<{
    key: typeof summaries;
    index: SpanIndex;
  } | null>(null);
  const getSpanIndex = useCallback((): SpanIndex => {
    if (spanIndexCacheRef.current?.key !== summaries) {
      spanIndexCacheRef.current = {
        key: summaries,
        index: buildSpanIndex(summaries),
      };
    }
    return spanIndexCacheRef.current.index;
  }, [summaries]);

  // One-time hydration of filters from URL (per summaries identity, i.e. per log).
  const filtersHydratedRef = useRef<EventSummaryStore | null>(null);
  if (filtersHydratedRef.current !== summaries) {
    filtersHydratedRef.current = summaries;
    const raw = readUrlList('f');
    if (raw.length > 0) {
      const decoded = raw
        .map(s => parseFilter(s, id => getDescendants(getSpanIndex(), id)))
        .filter((f): f is ActiveFilter => f != null);
      // Reset state synchronously during render — safe because this only
      // happens once per summaries identity (the ref guards it).
      setActiveFilters(decoded);
    } else if (activeFilters.length > 0) {
      setActiveFilters([]);
    }
  }

  // Sync filters → URL whenever they change (after hydration).
  useEffect(() => {
    writeUrlList('f', activeFilters.map(serializeFilter));
  }, [activeFilters]);

  // Apply custom filters + text search.
  // Returns either the original store (no filter/sort applied) or a FilteredView
  // backed by an Int32Array of indices — never copies the underlying data.
  const filteredEvents: SummaryView = useMemo(() => {
    const total = summaries.length;
    let indices: Int32Array | null = null;

    const needsFilter = activeFilters.length > 0 || !!textSearch;
    if (needsFilter) {
      const lower = textSearch.toLowerCase();
      const tmp: number[] = [];
      for (let i = 0; i < total; i++) {
        // Active field/span filters
        if (activeFilters.length > 0) {
          // Materialize only when we have filters that need it
          const evt = summaries.get(i);
          if (!evaluateFilters(evt, activeFilters, getEventData)) continue;
        }
        if (textSearch) {
          // Fast path: summary fields only
          const type = summaries.getType(i);
          const eventType = summaries.getEventType(i);
          const actionName = summaries.getActionName(i);
          const targetLabel = summaries.getTargetLabel(i);
          const quickMatch =
            type.toLowerCase().includes(lower) ||
            (eventType?.toLowerCase().includes(lower) ?? false) ||
            (actionName?.toLowerCase().includes(lower) ?? false) ||
            (targetLabel?.toLowerCase().includes(lower) ?? false);
          if (!quickMatch) {
            if (isLargeLog) continue;
            // Slow path: search raw proto bytes
            const evt = summaries.get(i);
            const raw = textDecoder.decode(getEventBytes(evt));
            if (!raw.toLowerCase().includes(lower)) continue;
          }
        }
        tmp.push(i);
      }
      indices = new Int32Array(tmp);
    }

    if (sortField) {
      // Build base indices if we don't have any from filtering
      if (!indices) {
        indices = new Int32Array(total);
        for (let i = 0; i < total; i++) indices[i] = i;
      }
      const dir = sortDir === 'asc' ? 1 : -1;
      const arr = Array.from(indices);
      arr.sort((ai, bi) => {
        const av = sortValueAt(summaries, ai, sortField);
        const bv = sortValueAt(summaries, bi, sortField);
        if (av < bv) return -1 * dir;
        if (av > bv) return 1 * dir;
        return 0;
      });
      indices = new Int32Array(arr);
    }

    if (indices) {
      return new FilteredView(summaries, indices);
    }
    return summaries;
  }, [
    summaries,
    activeFilters,
    textSearch,
    sortField,
    sortDir,
    getEventData,
    getEventBytes,
    isLargeLog,
  ]);

  // --- Sort actions ---

  const toggleSort = useCallback(
    (field: SortField) => {
      if (sortField === field) {
        setSortRaw(stringifySort(field, sortDir === 'asc' ? 'desc' : 'asc'));
      } else {
        setSortRaw(stringifySort(field, 'asc'));
      }
    },
    [sortField, sortDir, setSortRaw],
  );

  // --- Column visibility ---

  const isColumnVisible = useCallback(
    (id: string) => !hiddenColumns.has(id),
    [hiddenColumns],
  );

  const toggleColumn = useCallback(
    (id: string) => {
      const next = new Set(hiddenColumns);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      const nextStr = setToCols(next);
      setColsRaw(nextStr === DEFAULT_HIDDEN_STR ? DEFAULT_HIDDEN_STR : nextStr);
    },
    [hiddenColumns, setColsRaw],
  );

  // --- Filter actions ---

  const addFieldFilter = useCallback(
    (path: string, value: string, operator: FieldFilter['operator'] = 'eq') => {
      setActiveFilters(prev => {
        if (
          prev.some(
            f =>
              f.kind === 'field' &&
              f.path === path &&
              f.value === value &&
              f.operator === operator,
          )
        ) {
          return prev;
        }
        const shortPath = path.split('.').pop() ?? path;
        return [
          ...prev,
          {
            kind: 'field',
            id: nextFilterId(),
            path,
            operator,
            value,
            label: `${shortPath} ${operator === 'neq' ? '≠' : operator === 'contains' ? '~' : '='} ${value}`,
          },
        ];
      });
    },
    [],
  );

  const addSpanFilter = useCallback(
    (spanId: number, label: string) => {
      // Focus span replaces all other filters
      const descendants = getDescendants(getSpanIndex(), spanId);
      setActiveFilters([
        {
          kind: 'span',
          id: nextFilterId(),
          spanId,
          label,
          descendants,
        },
      ]);
      setTextSearch('');
    },
    [getSpanIndex],
  );

  const removeFilter = useCallback((id: string) => {
    setActiveFilters(prev => prev.filter(f => f.id !== id));
  }, []);

  const clearFilters = useCallback(() => {
    setActiveFilters([]);
    setTextSearch('');
  }, []);

  // --- Selection ---

  function findEventPositionByIndex(target: number): number {
    for (let i = 0; i < filteredEvents.length; i++) {
      if (filteredEvents.getIndex(i) === target) return i;
    }
    return -1;
  }

  const selectedEvent = useMemo(() => {
    if (selectedIndex == null) return null;
    const pos = findEventPositionByIndex(selectedIndex);
    return pos >= 0 ? filteredEvents.get(pos) : null;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filteredEvents, selectedIndex]);

  const selectEvent = useCallback(
    (index: number | null) => {
      setSelectedIndexRaw(index);
    },
    [setSelectedIndexRaw],
  );

  const selectPrev = useCallback(() => {
    if (selectedIndex == null) return;
    const pos = findEventPositionByIndex(selectedIndex);
    if (pos > 0) setSelectedIndexRaw(filteredEvents.getIndex(pos - 1));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filteredEvents, selectedIndex, setSelectedIndexRaw]);

  const selectNext = useCallback(() => {
    if (selectedIndex == null) return;
    const pos = findEventPositionByIndex(selectedIndex);
    if (pos >= 0 && pos < filteredEvents.length - 1) {
      setSelectedIndexRaw(filteredEvents.getIndex(pos + 1));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filteredEvents, selectedIndex, setSelectedIndexRaw]);

  return {
    filteredEvents,

    // Sort
    sortField,
    sortDir,
    toggleSort,

    // Column visibility
    isColumnVisible,
    toggleColumn,

    // Filters
    activeFilters,
    addFieldFilter,
    addSpanFilter,
    removeFilter,
    clearFilters,
    textSearch,
    setTextSearch,

    // Selection
    selectedEvent,
    selectedIndex,
    selectEvent,
    selectPrev,
    selectNext,

    // Span (lazy — only built on first call)
    getSpanIndex,
    /** Underlying summary store, exposed for span helpers that need to look
     *  up parentId/eventType for a span by reading its first event. */
    summaries,

    // View mode
    inlineDetails,
    setInlineDetails,

    // On-demand data access
    getEventData,

    // Large log mode
    isLargeLog,
  };
}

export type EventLogState = ReturnType<typeof useEventLogState>;
