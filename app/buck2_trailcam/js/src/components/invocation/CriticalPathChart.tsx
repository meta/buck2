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

import {useEffect, useState} from 'react';
import {Card, CardContent, CardHeader, CardTitle} from '../../ui';
import {useEventLog} from './EventLogProvider';
import PhaseSummaryPanel from './PhaseSummaryPanel';
import PathToggle, {type PathMode} from './PathToggle';
import {
  extractCriticalPathDataAsync,
  PHASE_COLORS,
  PHASE_LABELS,
  type CriticalPathData,
} from '../../lib/critical-path';
import {useUrlState} from '../../lib/url-state';

function fmtSec(ms: number): string {
  return `${(ms / 1000).toFixed(3)}s`;
}

function PlaceholderCard({message}: {message: string}) {
  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base">Critical Path</CardTitle>
      </CardHeader>
      <CardContent>
        <p className="text-muted-foreground text-sm">{message}</p>
      </CardContent>
    </Card>
  );
}

export default function CriticalPathChart() {
  const logState = useEventLog();
  const [mode, setMode] = useState<PathMode>('slowest');
  const [cpData, setCpData] = useState<CriticalPathData | null>(null);
  const [extractDone, setExtractDone] = useState(false);
  const [, setTab] = useUrlState('tab', 'overview');
  const [, setSubTab] = useUrlState('sub', 'load');

  function viewDetails() {
    setTab('performance');
    setSubTab('critical-path');
  }

  // Recompute whenever the event log state changes. We use the async
  // extractor so that for large logs we await the buildGraphInfo chunk
  // load (the sync extractor returns empty on LRU cache miss, which on
  // first paint produces a spurious "no data" until something else mounts
  // and warms the cache).
  useEffect(() => {
    if (logState.status !== 'loaded') {
      setCpData(null);
      setExtractDone(false);
      return;
    }
    if (logState.aggregates.criticalPath) {
      setCpData(logState.aggregates.criticalPath);
      setExtractDone(true);
      return;
    }
    let cancelled = false;
    setExtractDone(false);
    extractCriticalPathDataAsync(
      logState.summaries,
      logState.getEventDataAsync,
    ).then(result => {
      if (cancelled) return;
      setCpData(result);
      setExtractDone(true);
    });
    return () => {
      cancelled = true;
    };
  }, [logState]);

  if (logState.status === 'idle' || logState.status === 'loading') {
    return (
      <PlaceholderCard
        message={
          logState.status === 'loading' ? logState.progress : 'Loading...'
        }
      />
    );
  }

  if (logState.status === 'error') {
    return <PlaceholderCard message="No critical path data available." />;
  }

  if (!extractDone) {
    return <PlaceholderCard message="Computing critical path..." />;
  }

  if (
    !cpData ||
    (cpData.criticalPath.length === 0 && cpData.slowestPath.length === 0)
  ) {
    return <PlaceholderCard message="No critical path data available." />;
  }

  const groups = mode === 'critical' ? cpData.criticalPath : cpData.slowestPath;
  const total =
    mode === 'critical' ? cpData.totalDurationMs : cpData.slowestTotalMs;

  return (
    <Card>
      <CardHeader className="pb-2">
        <div className="flex items-center gap-3">
          <PathToggle
            value={mode}
            onValueChange={setMode}
            className="self-start"
          />
          <button
            onClick={viewDetails}
            className="group inline-flex h-7 items-center rounded-full bg-[var(--secondary)] p-[3px] text-[var(--foreground)]">
            <span className="inline-flex h-full items-center rounded-full px-3 text-sm font-normal transition-colors group-hover:bg-[var(--background)]/70">
              View details →
            </span>
          </button>
        </div>
      </CardHeader>
      <CardContent>
        {/* Horizontal stacked bar — same shape as before, fed from event-log
            phase totals instead of the GraphQL aggregate. */}
        <div className="flex h-8 w-full overflow-hidden rounded">
          {groups.map((g, i) => {
            const pct = (g.totalDurationMs / total) * 100;
            if (pct < 0.1) return null;
            const color = PHASE_COLORS[g.phase];
            return (
              <div
                key={i}
                style={{width: `${pct}%`, backgroundColor: color}}
                title={`${PHASE_LABELS[g.phase]}: ${fmtSec(g.totalDurationMs)} (${pct.toFixed(1)}%)`}
              />
            );
          })}
        </div>

        <p className="text-muted-foreground mt-2 text-xs">
          Total: {fmtSec(total)}
        </p>

        {/* Vertical stack of per-phase summary panels. The wrapper has a
            full rounded gray frame on all 4 sides; each row paints a
            colored "C" on top of that frame on the left side — curved
            cap at the first row's top-left, fading horizontal arm
            extending right, vertical bar down the left side, and the
            mirror at the last row's bottom-left.

            All the colored elements are shifted -1px so they overlap and
            cover the wrapper's gray border on the left/top-left/bottom-
            left. The curve radius (8px) matches the wrapper's rounded
            corners, so the cap's 3px arc cleanly hides the gray 1px arc
            at the rounded corners. Where the fade arm goes transparent
            on the right, the gray top/bottom border shows through. */}
        <div className="relative mt-4 rounded-lg border border-gray-400 dark:border-gray-500">
          {groups.map((g, i) => {
            const isFirst = i === 0;
            const isLast = i === groups.length - 1;
            const color = PHASE_COLORS[g.phase];
            return (
              <div key={i} className="relative">
                {/* Top-left curved cap + fading horizontal arm (first only) */}
                {isFirst && (
                  <>
                    <div
                      className="pointer-events-none absolute h-2 w-2"
                      style={{
                        top: '-1px',
                        left: '-1px',
                        borderTop: `3px solid ${color}`,
                        borderLeft: `3px solid ${color}`,
                        borderTopLeftRadius: '8px',
                      }}
                    />
                    <div
                      className="pointer-events-none absolute h-[3px] w-20"
                      style={{
                        top: '-1px',
                        left: '7px',
                        background: `linear-gradient(to right, ${color}, transparent)`,
                      }}
                    />
                  </>
                )}
                {/* Vertical phase bar — overlaps the wrapper's gray left
                    border (left:-1px), and leaves room at the top/bottom
                    of the first/last rows for the curved caps. */}
                <div
                  className="pointer-events-none absolute w-[3px]"
                  style={{
                    backgroundColor: color,
                    left: '-1px',
                    top: isFirst ? '7px' : '0',
                    bottom: isLast ? '7px' : '0',
                  }}
                />
                {/* Bottom-left curved cap + fading horizontal arm (last only) */}
                {isLast && (
                  <>
                    <div
                      className="pointer-events-none absolute h-2 w-2"
                      style={{
                        bottom: '-1px',
                        left: '-1px',
                        borderBottom: `3px solid ${color}`,
                        borderLeft: `3px solid ${color}`,
                        borderBottomLeftRadius: '8px',
                      }}
                    />
                    <div
                      className="pointer-events-none absolute h-[3px] w-20"
                      style={{
                        bottom: '-1px',
                        left: '7px',
                        background: `linear-gradient(to right, ${color}, transparent)`,
                      }}
                    />
                  </>
                )}
                <PhaseSummaryPanel group={g} totalMs={total} />
              </div>
            );
          })}
        </div>
      </CardContent>
    </Card>
  );
}
