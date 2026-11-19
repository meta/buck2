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
  PHASE_COLORS,
  PHASE_LABELS,
  type PhaseGroup,
} from '../../lib/critical-path';
import {
  aggregateGroup,
  formatAggregationKey,
  type AggregatedRow,
} from '../../lib/critical-path-aggregation';

function fmtSec(ms: number): string {
  return `${(ms / 1000).toFixed(3)}s`;
}

interface PhaseSummaryPanelProps {
  group: PhaseGroup;
  totalMs: number;
  /** Optional pre-computed aggregation; useful when the caller renders
   *  many panels and wants to compute once. */
  rows?: AggregatedRow[];
  /** Optional content to render in the top-right of the header (e.g.
   *  the collapse toggle in the details view). */
  headerAction?: React.ReactNode;
}

/**
 * The summary panel content used by the critical path details view (sticky,
 * left side of each phase section) and by the overview chart (stacked
 * vertically). Renders: phase title + total + percent, a proportional
 * progress bar, and a breakdown by aggregation key.
 */
export default function PhaseSummaryPanel({
  group,
  totalMs,
  rows,
  headerAction,
}: PhaseSummaryPanelProps) {
  const color = PHASE_COLORS[group.phase];
  const label = PHASE_LABELS[group.phase];
  const pct = totalMs > 0 ? (group.totalDurationMs / totalMs) * 100 : 0;
  const summary = rows ?? aggregateGroup(group);

  return (
    <div className="p-3">
      <div className="mb-2 flex items-baseline gap-3">
        <span className="w-16 shrink-0 whitespace-nowrap text-right text-lg font-bold">
          {fmtSec(group.totalDurationMs)}
        </span>
        <span className="w-14 shrink-0 whitespace-nowrap text-right text-sm font-bold">
          {pct.toFixed(1)}%
        </span>
        <span className="min-w-0 flex-1 text-lg font-bold" style={{color}}>
          {label}
        </span>
        {headerAction}
      </div>

      {/* Proportional bar */}
      <div className="mb-2 h-1.5 w-full rounded-full bg-gray-100 dark:bg-gray-800">
        <div
          className="h-full rounded-full"
          style={{backgroundColor: color, width: `${pct}%`}}
        />
      </div>

      {/* Breakdown by aggregation key. Fixed-width font-mono columns for
          duration / overall % / phase % so the digits line up across
          rows; label takes the remaining space and truncates. */}
      <div className="space-y-1 text-sm">
        {summary.map(row => {
          const isOverhead = row.key === 'waiting';
          const displayLabel = formatAggregationKey(row.key);
          const overallPct = totalMs > 0 ? (row.totalMs / totalMs) * 100 : 0;
          const phasePct =
            group.totalDurationMs > 0
              ? (row.totalMs / group.totalDurationMs) * 100
              : 0;
          const overallTitle = `${fmtSec(row.totalMs)} of ${fmtSec(totalMs)} total path time = ${overallPct.toFixed(1)}%`;
          const phaseTitle = `${fmtSec(row.totalMs)} of ${fmtSec(group.totalDurationMs)} ${label} phase time = ${phasePct.toFixed(1)}%`;
          return (
            <div key={row.key} className="flex items-center gap-3">
              <span
                className={`w-16 shrink-0 whitespace-nowrap text-right font-mono ${isOverhead ? 'text-muted-foreground' : ''}`}>
                {fmtSec(row.totalMs)}
              </span>
              <span
                className="text-muted-foreground w-14 shrink-0 whitespace-nowrap text-right font-mono"
                title={overallTitle}>
                {overallPct.toFixed(1)}%
              </span>
              <span
                className="w-20 shrink-0 whitespace-nowrap font-mono text-xs"
                style={{color}}
                title={phaseTitle}>
                ({phasePct.toFixed(1)}%)
              </span>
              <div className="text-muted-foreground flex min-w-0 flex-1 items-baseline gap-1.5">
                <span
                  className={`min-w-0 truncate ${isOverhead ? 'italic' : ''}`}
                  title={row.key}>
                  {displayLabel}
                </span>
                {row.count > 1 && (
                  <span className="shrink-0">({row.count}×)</span>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
