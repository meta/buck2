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

import {useRef} from 'react';
import {useVirtualizer} from '@tanstack/react-virtual';
import {Badge} from '../../../ui';
import type {EventLogState, SortField} from './useEventLogState';
import type {EventSummary} from '../../../lib/event-log-decoder';
import JsonTreeView from './JsonTreeView';
import {formatDuration} from '../../../lib/format';

const ROW_HEIGHT = 32;

function TypeBadge({type}: {type: string}) {
  const variant =
    type === 'spanStart'
      ? 'default'
      : type === 'spanEnd'
        ? 'secondary'
        : type === 'instant'
          ? 'outline'
          : type === 'result'
            ? 'destructive'
            : 'secondary';
  return (
    <Badge variant={variant} className="text-[10px] px-1 py-0">
      {type}
    </Badge>
  );
}

function formatTimestampMs(ms: number | undefined): string {
  if (ms == null) return '';
  const d = new Date(ms);
  return d.toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    fractionalSecondDigits: 3,
  } as Intl.DateTimeFormatOptions);
}

interface Column {
  id: SortField | 'spanId' | 'parentId';
  label: string;
  width: number;
  sortable: boolean;
  render: (event: EventSummary) => React.ReactNode;
}

const COLUMNS: Column[] = [
  {id: 'index', label: '#', width: 50, sortable: false, render: e => e.index},
  {
    id: 'timestampMs',
    label: 'Time',
    width: 110,
    sortable: true,
    render: e => formatTimestampMs(e.timestampMs),
  },
  {
    id: 'type',
    label: 'Type',
    width: 80,
    sortable: true,
    render: e => <TypeBadge type={e.type} />,
  },
  {
    id: 'eventType',
    label: 'Event',
    width: 160,
    sortable: true,
    render: e => <span className="font-mono">{e.eventType ?? ''}</span>,
  },
  {
    id: 'durationMs',
    label: 'Duration',
    width: 90,
    sortable: true,
    render: e => (e.durationMs != null ? formatDuration(e.durationMs) : ''),
  },
  {
    id: 'actionName',
    label: 'Action',
    width: 200,
    sortable: false,
    render: e => e.actionName ?? '',
  },
  {
    id: 'executionKind',
    label: 'Exec',
    width: 100,
    sortable: false,
    render: e => e.executionKind ?? '',
  },
  {
    id: 'targetLabel',
    label: 'Target',
    width: 250,
    sortable: false,
    render: e => e.targetLabel ?? '',
  },
  {
    id: 'spanId',
    label: 'Span',
    width: 80,
    sortable: false,
    render: e => e.spanId ?? '',
  },
  {
    id: 'parentId',
    label: 'Parent',
    width: 80,
    sortable: false,
    render: e => e.parentId ?? '',
  },
];

export default function EventLogTable({state}: {state: EventLogState}) {
  const {
    filteredEvents,
    selectedIndex,
    selectEvent,
    inlineDetails,
    activeFilters,
    addFieldFilter,
    getEventData,
    sortField,
    sortDir,
    toggleSort,
    isColumnVisible,
  } = state;
  const parentRef = useRef<HTMLDivElement>(null);

  const visibleColumns = COLUMNS.filter(c => isColumnVisible(c.id));

  const virtualizer = useVirtualizer({
    count: filteredEvents.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 20,
  });

  return (
    <div ref={parentRef} className="h-full overflow-auto">
      <table className="w-full text-xs">
        <thead className="sticky top-0 z-10 bg-gray-50 dark:bg-gray-900">
          <tr>
            {visibleColumns.map(col => (
              <th
                key={col.id}
                className="px-2 py-1.5 text-left font-medium whitespace-nowrap"
                style={{width: col.width}}>
                {col.sortable ? (
                  <div
                    className="cursor-pointer select-none hover:text-foreground"
                    onClick={() => toggleSort(col.id as SortField)}>
                    {col.label}
                    {sortField === col.id
                      ? sortDir === 'asc'
                        ? ' ↑'
                        : ' ↓'
                      : ''}
                  </div>
                ) : (
                  col.label
                )}
              </th>
            ))}
          </tr>
        </thead>
        <tbody
          style={{
            height: `${virtualizer.getTotalSize()}px`,
            position: 'relative',
          }}>
          {virtualizer.getVirtualItems().map(virtualRow => {
            const event = filteredEvents.get(virtualRow.index);
            const isSelected = event.index === selectedIndex;
            return (
              <tr
                key={event.index}
                data-index={virtualRow.index}
                // Only measure rows in inlineDetails mode (variable height).
                // For fixed-height rows the estimateSize is exact and
                // measuring caches a per-row entry that grows linearly with
                // visited rows.
                ref={
                  inlineDetails
                    ? node => virtualizer.measureElement(node)
                    : undefined
                }
                onClick={() => selectEvent(event.index)}
                className={`absolute left-0 w-full cursor-pointer border-b border-gray-100 dark:border-gray-800 hover:bg-gray-50 dark:hover:bg-gray-800 ${
                  isSelected ? 'bg-blue-50 dark:bg-blue-950' : ''
                }`}
                style={
                  inlineDetails
                    ? {transform: `translateY(${virtualRow.start}px)`}
                    : {
                        height: `${ROW_HEIGHT}px`,
                        transform: `translateY(${virtualRow.start}px)`,
                      }
                }>
                {inlineDetails ? (
                  <td colSpan={visibleColumns.length}>
                    <div>
                      <div className="flex items-center gap-2 px-2 py-1">
                        <TypeBadge type={event.type} />
                        <span className="font-mono text-gray-500">
                          {event.eventType ?? '—'}
                        </span>
                        {event.durationMs != null && (
                          <span className="text-muted-foreground">
                            {formatDuration(event.durationMs)}
                          </span>
                        )}
                        {event.targetLabel && (
                          <span className="text-muted-foreground truncate">
                            {event.targetLabel}
                          </span>
                        )}
                      </div>
                      <div className="border-t border-gray-100 px-2 py-1 dark:border-gray-800">
                        <JsonTreeView
                          data={getEventData(event)}
                          onFilter={addFieldFilter}
                          activeFilters={activeFilters}
                        />
                      </div>
                    </div>
                  </td>
                ) : (
                  visibleColumns.map(col => (
                    <td
                      key={col.id}
                      className="px-2 py-1 truncate"
                      style={{width: col.width}}>
                      {col.render(event)}
                    </td>
                  ))
                )}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
