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

import {useMemo, useRef, useState} from 'react';
import {useEventLog} from './EventLogProvider';
import type {TimelineLane, Span} from '../../lib/phase-timer';

const PHASE_COLORS: Record<string, string> = {
  // Clock-sync handshake — should appear at ~t=0 across every lane.
  handshake: '#0891b2',
  // `await reader.read()` on the response body in the decompress worker.
  // The end of the LAST network_read marks when the download finished.
  network_read: '#16a34a',
  decompress_chunk: '#a78bfa',
  dispatch_chunk: '#94a3b8',
  dispatch_batch: '#f59e0b',
  flush_chunk: '#ec4899',
  decode_batch: '#3b82f6',
  // Dispatcher tail
  await_last_decoders: '#fbbf24',
  drain_pending_writes: '#f97316',
  merge_partials: '#ef4444',
  write_worker_results: '#dc2626',
  // Main thread
  fetch: '#22c55e',
  await_decode: '#cbd5e1',
  post_decode: '#10b981',
};

function colorFor(phase: string): string {
  return PHASE_COLORS[phase] ?? '#6b7280';
}

function fmt(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms >= 10) return `${ms.toFixed(0)}ms`;
  return `${ms.toFixed(1)}ms`;
}

interface PreparedSpan {
  phase: string;
  /** Display-relative start (ms from earliest span across all lanes). */
  startMs: number;
  endMs: number;
  /** Original index of this span within its lane's `spans` array. */
  indexInLane: number;
  /** Annotations attached during the span (e.g. row counts). */
  attrs?: Record<string, string | number | boolean>;
}

interface PreparedLane {
  workerName: string;
  /** Spans translated to display-time (ms relative to the earliest start
   *  across all lanes). */
  spans: PreparedSpan[];
  /** Phase summary: total time per phase in this lane. */
  phaseTotals: Map<string, number>;
}

interface HoveredSpan {
  laneName: string;
  laneIndex: number;
  spanIndexInLane: number;
  laneSpanCount: number;
  phase: string;
  startMs: number;
  endMs: number;
  attrs?: Record<string, string | number | boolean>;
}

function prepareTimeline(lanes: TimelineLane[]): {
  prepared: PreparedLane[];
  totalSpanMs: number;
} {
  // Translate every span into a shared timeline using each lane's
  // offsetMs, then shift so the earliest start lands at 0.
  let earliest = Number.POSITIVE_INFINITY;
  let latest = 0;
  for (const lane of lanes) {
    for (const s of lane.spans) {
      const start = s.startMs + lane.offsetMs;
      const end = s.endMs + lane.offsetMs;
      if (start < earliest) earliest = start;
      if (end > latest) latest = end;
    }
  }
  if (!isFinite(earliest)) earliest = 0;
  const totalSpanMs = Math.max(0, latest - earliest);

  const prepared: PreparedLane[] = lanes.map(lane => {
    const phaseTotals = new Map<string, number>();
    const spans = lane.spans.map((s: Span, indexInLane: number) => {
      const startMs = s.startMs + lane.offsetMs - earliest;
      const endMs = s.endMs + lane.offsetMs - earliest;
      phaseTotals.set(
        s.phase,
        (phaseTotals.get(s.phase) ?? 0) + (endMs - startMs),
      );
      return {phase: s.phase, startMs, endMs, indexInLane, attrs: s.attrs};
    });
    return {workerName: lane.workerName, spans, phaseTotals};
  });
  return {prepared, totalSpanMs};
}

/**
 * Swim-lane visualization of the parallel decode pipeline. One lane per
 * worker, time axis along the bottom, each span rendered as a colored
 * absolute-positioned rect. Click on a span to log its details.
 *
 * Renders only when timelineLanes is present (parallel-mode loaded
 * state); returns null otherwise.
 */
export default function DecodeTimeline() {
  const logState = useEventLog();
  const lanes =
    logState.status === 'loaded' ? logState.timelineLanes : undefined;
  const [hovered, setHovered] = useState<HoveredSpan | null>(null);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [tooltipPos, setTooltipPos] = useState<{x: number; y: number} | null>(
    null,
  );

  const {prepared, totalSpanMs} = useMemo(() => {
    if (!lanes || lanes.length === 0) return {prepared: [], totalSpanMs: 0};
    return prepareTimeline(lanes);
  }, [lanes]);

  if (!lanes || prepared.length === 0 || totalSpanMs <= 0) return null;

  // Distinct phases used in any lane, for the legend below.
  const allPhases = new Set<string>();
  for (const lane of prepared)
    for (const s of lane.spans) allPhases.add(s.phase);

  const LANE_HEIGHT = 18;
  const LANE_GAP = 4;
  const LABEL_WIDTH = 96;

  return (
    <div
      ref={wrapperRef}
      className="relative rounded-lg border border-gray-200 bg-gray-50 p-3 text-xs dark:border-gray-800 dark:bg-gray-900/40"
      onMouseLeave={() => {
        setHovered(null);
        setTooltipPos(null);
      }}>
      <div className="mb-2 flex items-center justify-between">
        <div className="font-medium">Decode timeline</div>
        <div className="text-muted-foreground font-mono">
          {fmt(totalSpanMs)} total
        </div>
      </div>
      <div className="relative" style={{paddingLeft: LABEL_WIDTH}}>
        {prepared.map((lane, laneIndex) => (
          <div
            key={lane.workerName}
            className="relative"
            style={{
              height: LANE_HEIGHT,
              marginBottom: LANE_GAP,
            }}>
            <div
              className="text-muted-foreground absolute right-full mr-2 text-[11px] font-mono whitespace-nowrap"
              style={{top: 1, lineHeight: `${LANE_HEIGHT}px`}}>
              {lane.workerName}
            </div>
            <div className="relative h-full overflow-hidden rounded bg-gray-200/60 dark:bg-gray-800/60">
              {lane.spans.map((s, i) => {
                const left = (s.startMs / totalSpanMs) * 100;
                const width = Math.max(
                  0.05,
                  ((s.endMs - s.startMs) / totalSpanMs) * 100,
                );
                const isHovered =
                  hovered &&
                  hovered.laneIndex === laneIndex &&
                  hovered.spanIndexInLane === s.indexInLane;
                return (
                  <div
                    key={i}
                    className="absolute top-0 h-full cursor-default transition-opacity"
                    style={{
                      left: `${left}%`,
                      width: `${width}%`,
                      backgroundColor: colorFor(s.phase),
                      // Avoid hairline boxes from disappearing entirely
                      minWidth: 1,
                      outline: isHovered ? '1px solid currentColor' : undefined,
                      outlineOffset: isHovered ? -1 : undefined,
                      opacity: hovered && !isHovered ? 0.5 : 1,
                    }}
                    onMouseEnter={e => {
                      setHovered({
                        laneName: lane.workerName,
                        laneIndex,
                        spanIndexInLane: s.indexInLane,
                        laneSpanCount: lane.spans.length,
                        phase: s.phase,
                        startMs: s.startMs,
                        endMs: s.endMs,
                        attrs: s.attrs,
                      });
                      // Pin the tooltip to the entry point and don't
                      // follow the cursor afterwards — otherwise moving
                      // toward the tooltip makes it slide away and the
                      // user can never reach it to select text.
                      const rect = wrapperRef.current?.getBoundingClientRect();
                      if (rect) {
                        setTooltipPos({
                          x: e.clientX - rect.left,
                          y: e.clientY - rect.top,
                        });
                      }
                    }}
                  />
                );
              })}
            </div>
            {/* Subtle row count badge after the bar */}
            <div
              className="text-muted-foreground absolute right-0 top-0 pr-1 text-[10px] font-mono"
              style={{lineHeight: `${LANE_HEIGHT}px`}}>
              {lane.spans.length}
            </div>
          </div>
        ))}
      </div>
      {/* Legend */}
      <div className="text-muted-foreground mt-2 flex flex-wrap gap-x-3 gap-y-1 text-[11px]">
        {[...allPhases].sort().map(phase => (
          <span key={phase} className="inline-flex items-center gap-1">
            <span
              className="inline-block size-2 rounded-sm"
              style={{backgroundColor: colorFor(phase)}}
            />
            {phase}
          </span>
        ))}
      </div>

      {hovered && tooltipPos && (
        <SpanTooltip
          hovered={hovered}
          totalSpanMs={totalSpanMs}
          x={tooltipPos.x}
          y={tooltipPos.y}
        />
      )}
    </div>
  );
}

function SpanTooltip({
  hovered,
  totalSpanMs,
  x,
  y,
}: {
  hovered: HoveredSpan;
  totalSpanMs: number;
  x: number;
  y: number;
}) {
  const duration = hovered.endMs - hovered.startMs;
  const pct = totalSpanMs > 0 ? (duration / totalSpanMs) * 100 : 0;
  return (
    <div
      // pointer-events enabled so the user can hover into the tooltip to
      // select / copy the attribute values. The wrapper's onMouseLeave is
      // the only thing that clears hover state, so moving from a span
      // into the tooltip (both children of the wrapper) keeps it open.
      className="absolute z-50 cursor-text select-text rounded-md border border-gray-300 bg-white px-2.5 py-2 text-[11px] shadow-lg dark:border-gray-700 dark:bg-gray-900"
      style={{
        // Offset from cursor; clamp at the right edge by translating
        // up-and-right out of the cursor's path.
        left: x + 12,
        top: y + 12,
        transform: 'translate(0, 0)',
        // Keep tooltip within parent — naive clamp: if x is past 70% of
        // parent width, anchor to the cursor's left side instead.
        maxWidth: 280,
      }}>
      <div className="mb-1 flex items-center gap-1.5">
        <span
          className="inline-block size-2.5 shrink-0 rounded-sm"
          style={{backgroundColor: colorFor(hovered.phase)}}
        />
        <span className="font-mono font-medium">{hovered.phase}</span>
      </div>
      <div className="text-muted-foreground space-y-0.5 font-mono">
        <div>
          <span className="text-muted-foreground">lane:</span>{' '}
          <span className="text-foreground">{hovered.laneName}</span>
        </div>
        <div>
          <span className="text-muted-foreground">span:</span>{' '}
          <span className="text-foreground">
            {hovered.spanIndexInLane + 1} / {hovered.laneSpanCount}
          </span>
        </div>
        <div>
          <span className="text-muted-foreground">start:</span>{' '}
          <span className="text-foreground">{fmt(hovered.startMs)}</span>
        </div>
        <div>
          <span className="text-muted-foreground">end:</span>{' '}
          <span className="text-foreground">{fmt(hovered.endMs)}</span>
        </div>
        <div>
          <span className="text-muted-foreground">duration:</span>{' '}
          <span className="text-foreground font-medium">{fmt(duration)}</span>{' '}
          <span className="text-muted-foreground">({pct.toFixed(2)}%)</span>
        </div>
      </div>
      {hovered.attrs && Object.keys(hovered.attrs).length > 0 && (
        <div className="text-muted-foreground mt-1.5 space-y-0.5 border-t border-gray-200 pt-1.5 font-mono dark:border-gray-700">
          {Object.entries(hovered.attrs).map(([k, v]) => (
            <div key={k}>
              <span className="text-muted-foreground">{k}:</span>{' '}
              <span className="text-foreground">{String(v)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
