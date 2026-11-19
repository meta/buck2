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

import {ResizablePanelGroup, ResizablePanel, ResizableHandle} from '../../ui';
import {useEventLog} from './EventLogProvider';
import {useEventLogState} from './event-log/useEventLogState';
import EventLogTable from './event-log/EventLogTable';
import EventLogToolbar from './event-log/EventLogToolbar';
import EventLogDetailPanel from './event-log/EventLogDetailPanel';

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export default function EventLogViewer() {
  const logState = useEventLog();

  if (logState.status === 'idle' || logState.status === 'loading') {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-12">
        <p className="text-sm">
          {logState.status === 'loading' ? logState.progress : 'Loading...'}
        </p>
      </div>
    );
  }

  if (logState.status === 'error') {
    return (
      <div className="rounded border border-red-200 bg-red-50 p-4 dark:border-red-800 dark:bg-red-950">
        <p className="text-sm text-red-700 dark:text-red-300">
          Failed to load event log: {logState.message}
        </p>
      </div>
    );
  }

  return (
    <EventLogViewerLoaded
      summaries={logState.summaries}
      getEventData={logState.getEventData}
      getEventBytes={logState.getEventBytes}
      rawSize={logState.rawSize}
      decompressedSize={logState.decompressedSize}
      fetchDecodeMs={logState.fetchDecodeMs}
      isLargeLog={logState.isLargeLog}
    />
  );
}

function fmtMs(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  return `${ms.toFixed(0)}ms`;
}

function fmtRate(eventsPerSec: number): string {
  if (!isFinite(eventsPerSec) || eventsPerSec <= 0) return '—';
  if (eventsPerSec >= 1000)
    return `${(eventsPerSec / 1000).toFixed(1)}k events/s`;
  return `${eventsPerSec.toFixed(0)} events/s`;
}

function EventLogViewerLoaded({
  summaries,
  getEventData,
  getEventBytes,
  rawSize,
  decompressedSize,
  fetchDecodeMs,
  isLargeLog,
}: {
  summaries: import('../../lib/event-summary-store').EventSummaryStore;
  getEventData: (
    s: import('../../lib/event-log-decoder').EventSummary,
  ) => Record<string, unknown>;
  getEventBytes: (
    s: import('../../lib/event-log-decoder').EventSummary,
  ) => Uint8Array;
  rawSize: number;
  decompressedSize: number;
  fetchDecodeMs: number;
  isLargeLog: boolean;
}) {
  const state = useEventLogState(
    summaries,
    getEventData,
    getEventBytes,
    isLargeLog,
  );
  const eventsPerSec =
    fetchDecodeMs > 0 ? (summaries.length * 1000) / fetchDecodeMs : 0;

  return (
    <div className="flex h-full flex-col">
      {/* Stats bar */}
      <div className="text-muted-foreground mb-2 flex shrink-0 items-center gap-4 text-xs">
        <span>
          {formatSize(rawSize)} → {formatSize(decompressedSize)}
        </span>
        <span>{summaries.length.toLocaleString()} total events</span>
        {fetchDecodeMs > 0 && (
          <>
            <span>fetch + decode {fmtMs(fetchDecodeMs)}</span>
            {eventsPerSec > 0 && <span>{fmtRate(eventsPerSec)}</span>}
          </>
        )}
      </div>

      {/* Toolbar */}
      <div className="shrink-0">
        <EventLogToolbar state={state} />
      </div>

      {/* Table + Detail panel */}
      <div className="mt-2 min-h-0 flex-1">
        {state.selectedEvent ? (
          <ResizablePanelGroup direction="horizontal">
            <ResizablePanel defaultSize={60} minSize={30}>
              <div className="h-full overflow-hidden rounded border">
                <EventLogTable state={state} />
              </div>
            </ResizablePanel>
            <ResizableHandle withHandle />
            <ResizablePanel defaultSize={40} minSize={20}>
              <div className="h-full overflow-hidden rounded border">
                <EventLogDetailPanel state={state} />
              </div>
            </ResizablePanel>
          </ResizablePanelGroup>
        ) : (
          <div className="h-full overflow-hidden rounded border">
            <EventLogTable state={state} />
          </div>
        )}
      </div>
    </div>
  );
}
