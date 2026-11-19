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

import {useMemo, useState, useEffect} from 'react';
import {Badge, Chip, Input} from '../../../ui';
import type {EventLogState} from './useEventLogState';
import {TOGGLEABLE_COLUMNS} from './useEventLogState';
import {getSpanAncestry, getSpanType} from './span-index';

export default function EventLogToolbar({state}: {state: EventLogState}) {
  const {
    filteredEvents,
    activeFilters,
    removeFilter,
    clearFilters,
    textSearch,
    setTextSearch,
    addFieldFilter,
    addSpanFilter,
    getSpanIndex,
    summaries,
    inlineDetails,
    setInlineDetails,
    isColumnVisible,
    toggleColumn,
  } = state;

  // Debounced search
  const [searchInput, setSearchInput] = useState(textSearch);
  useEffect(() => {
    const t = setTimeout(() => setTextSearch(searchInput), 200);
    return () => clearTimeout(t);
  }, [searchInput, setTextSearch]);

  // Collect unique eventType values with counts for the dropdown
  const eventTypeCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (let i = 0; i < filteredEvents.length; i++) {
      const eventType = filteredEvents.getEventType(i);
      if (eventType) {
        counts.set(eventType, (counts.get(eventType) ?? 0) + 1);
      }
    }
    return Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  }, [filteredEvents]);

  // Get span filter for breadcrumbs
  const spanFilter = activeFilters.find(f => f.kind === 'span');
  const spanBreadcrumbs = useMemo(() => {
    if (!spanFilter || spanFilter.kind !== 'span') return null;
    return getSpanAncestry(getSpanIndex(), summaries, spanFilter.spanId);
  }, [spanFilter, getSpanIndex, summaries]);

  // Simple event type dropdown
  const [showEventTypes, setShowEventTypes] = useState(false);

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        {/* Active filter chips */}
        {activeFilters
          .filter(f => f.kind === 'field')
          .map(filter => (
            <Chip
              key={filter.id}
              variant="filter"
              size="sm"
              onRemove={() => removeFilter(filter.id)}>
              {filter.label}
            </Chip>
          ))}

        {/* Span breadcrumbs */}
        {spanBreadcrumbs && spanFilter && spanFilter.kind === 'span' && (
          <div className="flex items-center gap-0.5">
            {spanBreadcrumbs.map((crumb, i) => (
              <span key={crumb.spanId} className="flex items-center gap-0.5">
                {i > 0 && (
                  <span className="text-muted-foreground text-xs">›</span>
                )}
                <Chip
                  variant={
                    crumb.spanId === spanFilter.spanId ? 'filter' : 'outline'
                  }
                  size="sm"
                  selected={crumb.spanId === spanFilter.spanId}
                  onChipClick={() => {
                    const type =
                      getSpanType(getSpanIndex(), summaries, crumb.spanId) ??
                      'span';
                    addSpanFilter(crumb.spanId, type);
                  }}
                  onRemove={
                    crumb.spanId === spanFilter.spanId
                      ? () => removeFilter(spanFilter.id)
                      : undefined
                  }>
                  {crumb.eventType}
                </Chip>
              </span>
            ))}
          </div>
        )}

        {activeFilters.length > 0 && (
          <button
            onClick={clearFilters}
            className="text-muted-foreground text-xs hover:underline">
            Clear all
          </button>
        )}

        {/* Event type quick filter */}
        <div className="relative ml-auto">
          <button
            onClick={() => setShowEventTypes(!showEventTypes)}
            className="rounded border px-2 py-1 text-xs hover:bg-gray-50 dark:hover:bg-gray-800">
            + Event Type
          </button>
          {showEventTypes && (
            <>
              <div
                className="fixed inset-0 z-20"
                onClick={() => setShowEventTypes(false)}
              />
              <div className="absolute right-0 z-30 mt-1 max-h-64 w-56 overflow-auto rounded border bg-white shadow-lg dark:bg-gray-900">
                {eventTypeCounts.map(([type, count]) => (
                  <button
                    key={type}
                    onClick={() => {
                      addFieldFilter('eventType', type);
                      setShowEventTypes(false);
                    }}
                    className="flex w-full items-center justify-between px-3 py-1.5 text-xs hover:bg-gray-50 dark:hover:bg-gray-800">
                    <span className="font-mono">{type}</span>
                    <Badge variant="secondary" className="text-[10px]">
                      {count}
                    </Badge>
                  </button>
                ))}
              </div>
            </>
          )}
        </div>

        {/* Text search */}
        <Input
          type="text"
          placeholder="Search..."
          value={searchInput}
          onChange={e => setSearchInput(e.target.value)}
          className="h-7 w-48 text-xs"
        />

        {/* Inline details toggle */}
        <label className="flex shrink-0 cursor-pointer items-center gap-1.5 text-xs">
          <input
            type="checkbox"
            checked={inlineDetails}
            onChange={() => setInlineDetails(!inlineDetails)}
            className="accent-blue-600"
          />
          <span className="text-muted-foreground select-none">
            Show events inline
          </span>
        </label>

        {/* Column toggle */}
        <div className="flex shrink-0 items-center gap-2">
          {TOGGLEABLE_COLUMNS.map(col => (
            <label
              key={col.id}
              className="flex cursor-pointer items-center gap-1 text-xs">
              <input
                type="checkbox"
                checked={isColumnVisible(col.id)}
                onChange={() => toggleColumn(col.id)}
                className="accent-blue-600"
              />
              <span className="text-muted-foreground select-none">
                {col.label}
              </span>
            </label>
          ))}
        </div>

        {/* Count */}
        <Badge variant="secondary" className="text-xs shrink-0">
          {filteredEvents.length} events
        </Badge>
      </div>
    </div>
  );
}
