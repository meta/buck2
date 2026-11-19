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

import {useEffect, useMemo, useState} from 'react';
import {Card, CardContent, CardHeader, CardTitle} from '../../ui';
import {useEventLog, type EventLogState} from './EventLogProvider';
import {
  classifyStatus,
  flattenAction,
  type ActionDetail,
} from '../../lib/action-results';
import {AnsiOutput} from '../ui/AnsiOutput';
import {useUrlState} from '../../lib/url-state';

interface FailedActionEntry {
  storeIdx: number;
  target: string;
  /** Action name's first space-separated token (e.g. "cxx_compile"). */
  category: string;
  /** Everything after the first space — may itself contain spaces. */
  identifier: string;
}

/**
 * Overview-page card listing the failed actions in this invocation.
 *
 * Each row shows the action's identity in a single line; clicking expands
 * inline to lazy-load the proto and surface the most useful diagnostic
 * (stderr, with exit code if non-zero). For everything else (full
 * stdout, repro command, etc.) the user can hop to the Actions tab.
 *
 * Renders nothing while the event log is still loading or when there are
 * no failed actions, so the card is always quietly absent for green
 * builds and quietly informative for red ones.
 */
export default function FailedActionsCard() {
  const logState = useEventLog();
  if (logState.status !== 'loaded') return null;
  return <FailedActionsView logState={logState} />;
}

function FailedActionsView({
  logState,
}: {
  logState: Extract<EventLogState, {status: 'loaded'}>;
}) {
  const [, setTab] = useUrlState('tab', 'overview');
  const failed = useMemo(() => {
    const summaries = logState.summaries;
    const out: FailedActionEntry[] = [];
    for (let i = 0; i < summaries.length; i++) {
      if (summaries.getEventType(i) !== 'actionExecution') continue;
      if (summaries.getType(i) !== 'spanEnd') continue;
      const kind = summaries.getExecutionKind(i) ?? '';
      const status = classifyStatus(summaries.getFailed(i), kind);
      if (status !== 'failed') continue;
      const target = summaries.getTargetLabel(i) ?? '';
      const actionName = summaries.getActionName(i) ?? '';
      const sp = actionName.indexOf(' ');
      const category = sp >= 0 ? actionName.slice(0, sp) : actionName;
      const identifier = sp >= 0 ? actionName.slice(sp + 1) : '';
      out.push({storeIdx: i, target, category, identifier});
    }
    return out;
  }, [logState.summaries]);

  if (failed.length === 0) return null;

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-base">
          Failed Actions{' '}
          <span className="text-muted-foreground text-sm font-normal">
            ({failed.length})
          </span>
          {/* Same pill-button shape and placement (next to the title with
              `ml-3`) as TestSummaryCard's View details. Jumps to the
              Actions tab — that view's status preset already defaults to
              "Failed" when there are any failures. */}
          <button
            onClick={() => setTab('actions')}
            className="group ml-3 inline-flex h-7 items-center rounded-full bg-[var(--secondary)] p-[3px] text-[var(--foreground)]">
            <span className="inline-flex h-full items-center rounded-full px-3 text-sm font-normal transition-colors group-hover:bg-[var(--background)]/70">
              View details →
            </span>
          </button>
        </CardTitle>
      </CardHeader>
      <CardContent className="space-y-1">
        {failed.map(a => (
          <FailedActionRow key={a.storeIdx} entry={a} logState={logState} />
        ))}
      </CardContent>
    </Card>
  );
}

function FailedActionRow({
  entry,
  logState,
}: {
  entry: FailedActionEntry;
  logState: Extract<EventLogState, {status: 'loaded'}>;
}) {
  const [expanded, setExpanded] = useState(false);
  const [detail, setDetail] = useState<ActionDetail | null>(null);

  useEffect(() => {
    if (!expanded || detail) return;
    let cancelled = false;
    const summary = logState.summaries.get(entry.storeIdx);
    logState.getEventDataAsync(summary).then(data => {
      if (cancelled) return;
      const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
      const ae = spanEnd?.actionExecution as
        Record<string, unknown> | undefined;
      setDetail(flattenAction(ae));
    });
    return () => {
      cancelled = true;
    };
  }, [expanded, detail, logState, entry.storeIdx]);

  return (
    <div className="rounded border border-red-200 bg-red-50 dark:border-red-800 dark:bg-red-950">
      <button
        onClick={() => setExpanded(v => !v)}
        className="flex w-full items-start gap-2 px-2 py-1.5 text-left text-xs text-red-800 dark:text-red-200">
        <span className="mt-0.5 shrink-0 text-[10px] opacity-70">
          {expanded ? '▾' : '▸'}
        </span>
        <span className="min-w-0 flex-1 truncate font-mono">
          {entry.target || '(unknown target)'}
          {(entry.category || entry.identifier) && (
            <span className="ml-2 text-[11px] opacity-70">
              ({[entry.category, entry.identifier].filter(Boolean).join(' ')})
            </span>
          )}
        </span>
      </button>
      {expanded && (
        <div className="border-t border-red-200 px-2 py-2 text-xs dark:border-red-800">
          {!detail && (
            <p className="text-muted-foreground italic">Loading details…</p>
          )}
          {detail && (
            <div className="space-y-2">
              {detail.exitCode != null && detail.exitCode !== 0 && (
                <div>
                  <span className="text-muted-foreground">Exit code: </span>
                  <span className="font-mono">{detail.exitCode}</span>
                </div>
              )}
              {detail.stderr ? (
                <div className="overflow-auto rounded bg-amber-50 p-2 font-mono text-[11px] whitespace-pre-wrap dark:bg-amber-950">
                  <AnsiOutput>{detail.stderr}</AnsiOutput>
                </div>
              ) : (
                <p className="text-muted-foreground italic">No stderr</p>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
