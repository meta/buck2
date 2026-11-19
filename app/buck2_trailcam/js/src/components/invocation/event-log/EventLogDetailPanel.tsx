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

import {
  ArrowUpIcon,
  ArrowDownIcon,
  FocusIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CopyIcon,
} from 'lucide-react';
import {Badge, Button} from '../../../ui';
import JsonTreeView from './JsonTreeView';
import type {EventLogState} from './useEventLogState';
import {findSpanStartEvent, getSpanType} from './span-index';

export default function EventLogDetailPanel({state}: {state: EventLogState}) {
  const {
    selectedEvent,
    selectEvent,
    selectPrev,
    selectNext,
    addFieldFilter,
    addSpanFilter,
    activeFilters,
    getSpanIndex,
    summaries,
    getEventData,
  } = state;

  if (!selectedEvent) return null;

  const spanId = selectedEvent.spanId;
  const parentId = selectedEvent.parentId;
  // Lazy: building span index here is only triggered when an event is selected
  const spanIndex = spanId != null || parentId != null ? getSpanIndex() : null;
  const children =
    spanId != null && spanIndex ? spanIndex.childrenOf.get(spanId) : undefined;

  const eventData = selectedEvent ? getEventData(selectedEvent) : null;

  function formatTimestampMs(ms?: number): string {
    if (ms == null) return '';
    const d = new Date(ms);
    return d.toLocaleTimeString([], {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      fractionalSecondDigits: 3,
    } as Intl.DateTimeFormatOptions);
  }

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="shrink-0 border-b bg-gray-50 p-3 space-y-2 dark:bg-gray-900">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span className="text-muted-foreground text-xs">
              #{selectedEvent.index}
            </span>
            <Badge
              variant={
                selectedEvent.type === 'spanStart'
                  ? 'default'
                  : selectedEvent.type === 'spanEnd'
                    ? 'secondary'
                    : selectedEvent.type === 'instant'
                      ? 'outline'
                      : selectedEvent.type === 'result'
                        ? 'destructive'
                        : 'secondary'
              }
              className="text-[10px]">
              {selectedEvent.type}
            </Badge>
            {selectedEvent.eventType && (
              <span className="font-mono text-sm font-medium">
                {selectedEvent.eventType}
              </span>
            )}
          </div>
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              onClick={selectPrev}
              title="Previous">
              <ChevronLeftIcon className="size-4" />
            </Button>
            <Button variant="ghost" size="sm" onClick={selectNext} title="Next">
              <ChevronRightIcon className="size-4" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                navigator.clipboard.writeText(
                  JSON.stringify(eventData, null, 2),
                );
              }}
              title="Copy JSON">
              <CopyIcon className="size-4" />
            </Button>
          </div>
        </div>
        {selectedEvent.timestampMs && (
          <span className="text-muted-foreground text-xs font-mono">
            {formatTimestampMs(selectedEvent.timestampMs)}
          </span>
        )}

        {/* Span navigation */}
        {spanId != null && (
          <div className="flex flex-wrap items-center gap-1.5">
            {parentId != null && (
              <Button
                variant="outline"
                size="sm"
                className="h-6 text-xs"
                onClick={() => {
                  const idx = findSpanStartEvent(spanIndex!, parentId);
                  if (idx != null) selectEvent(idx);
                }}>
                <ArrowUpIcon className="size-3 mr-1" />
                Parent
                <span className="text-muted-foreground ml-1">
                  ({getSpanType(spanIndex!, summaries, parentId) ?? '?'})
                </span>
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              className="h-6 text-xs"
              onClick={() => {
                const type =
                  getSpanType(getSpanIndex(), summaries, spanId) ?? 'span';
                addSpanFilter(spanId, type);
              }}>
              <FocusIcon className="size-3 mr-1" />
              Focus span
            </Button>
          </div>
        )}

        {/* Child spans */}
        {children && children.length > 0 && (
          <div className="space-y-1">
            <span className="text-muted-foreground text-[10px] uppercase tracking-wide">
              Children ({children.length})
            </span>
            <div className="flex flex-wrap gap-1">
              {Array.from(children)
                .slice(0, 20)
                .map(childId => {
                  const type =
                    getSpanType(spanIndex!, summaries, childId) ?? '?';
                  return (
                    <button
                      key={childId}
                      onClick={() => {
                        const idx = findSpanStartEvent(spanIndex!, childId);
                        if (idx != null) selectEvent(idx);
                      }}
                      className="rounded bg-gray-100 px-1.5 py-0.5 text-[10px] font-mono hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700">
                      <ArrowDownIcon className="inline size-2.5 mr-0.5" />
                      {type}
                    </button>
                  );
                })}
              {children.length > 20 && (
                <span className="text-muted-foreground text-[10px]">
                  +{children.length - 20} more
                </span>
              )}
            </div>
          </div>
        )}
      </div>

      {/* JSON tree body */}
      <div className="flex-1 overflow-auto p-3 text-xs">
        <JsonTreeView
          data={eventData}
          onFilter={addFieldFilter}
          activeFilters={activeFilters}
        />
      </div>
    </div>
  );
}
