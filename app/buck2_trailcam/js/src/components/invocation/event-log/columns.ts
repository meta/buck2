/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {createColumnHelper} from '@tanstack/react-table';
import type {EventSummary} from '../../../lib/event-log-decoder';
import {formatDuration} from '../../../lib/format';

const col = createColumnHelper<EventSummary>();

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

// --- Column Definitions ---
// All fields are pre-extracted at decode time on DecodedEvent,
// so accessors are simple property reads — no deep traversals.

export const allColumns = [
  col.accessor('index', {
    header: '#',
    size: 50,
    enableHiding: false,
  }),

  col.accessor('timestampMs', {
    header: 'Time',
    size: 110,
    cell: info => formatTimestampMs(info.getValue()),
    sortingFn: (a, b) =>
      (a.original.timestampMs ?? 0) - (b.original.timestampMs ?? 0),
    enableHiding: false,
  }),

  col.accessor('type', {
    header: 'Type',
    size: 80,
    enableColumnFilter: true,
    filterFn: 'arrIncludesSome',
    enableHiding: false,
  }),

  col.accessor('eventType', {
    header: 'Event',
    size: 160,
    enableColumnFilter: true,
    filterFn: 'arrIncludesSome',
    enableHiding: false,
  }),

  col.accessor('durationMs', {
    header: 'Duration',
    size: 90,
    cell: info => {
      const ms = info.getValue();
      return ms != null ? formatDuration(ms) : '';
    },
    sortingFn: (a, b) =>
      (a.original.durationMs ?? 0) - (b.original.durationMs ?? 0),
    enableSorting: true,
  }),

  col.accessor('actionName', {
    header: 'Action',
    size: 200,
    cell: info => info.getValue() ?? '',
  }),

  col.accessor('executionKind', {
    header: 'Exec',
    size: 100,
    cell: info => info.getValue() ?? '',
  }),

  col.accessor('targetLabel', {
    header: 'Target',
    size: 250,
    cell: info => info.getValue() ?? '',
  }),

  col.accessor('spanId', {
    header: 'Span',
    size: 80,
    enableHiding: true,
  }),

  col.accessor('parentId', {
    header: 'Parent',
    size: 80,
    enableHiding: true,
  }),
];

export const defaultColumnVisibility: Record<string, boolean> = {
  spanId: false,
  parentId: false,
};
