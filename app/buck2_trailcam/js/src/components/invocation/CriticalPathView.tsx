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

import {useState, useRef, useEffect} from 'react';
import {Badge} from '../../ui';
import {useEventLog} from './EventLogProvider';
import PhaseSummaryPanel from './PhaseSummaryPanel';
import PathToggle, {type PathMode} from './PathToggle';
import {
  extractCriticalPathDataAsync,
  PHASE_COLORS,
  PHASE_LABELS,
  type CriticalPathData,
  type CriticalPathEntry,
  type PhaseGroup,
} from '../../lib/critical-path';
import {useUrlState} from '../../lib/url-state';
/** Format milliseconds as seconds with 3 decimal places (e.g. "1.234s") */
function fmtSec(ms: number): string {
  return `${(ms / 1000).toFixed(3)}s`;
}

function ExecutionKindBadge({kind}: {kind: string}) {
  const short = kind.replace('ACTION_EXECUTION_KIND_', '').replace(/_/g, ' ');
  const variant =
    short === 'LOCAL'
      ? 'default'
      : short === 'REMOTE'
        ? 'secondary'
        : short === 'ACTION CACHE'
          ? 'outline'
          : 'secondary';
  return (
    <Badge variant={variant} className="text-[10px] px-1 py-0 shrink-0">
      {short}
    </Badge>
  );
}

function TypeBadge({type}: {type: string}) {
  const labels: Record<string, string> = {
    action_execution: 'action',
    analysis: 'analysis',
    load: 'load',
    listing: 'listing',
    waiting: 'overhead',
    generic_entry: 'overhead',
    final_materialization: 'materialize',
    test_execution: 'test',
    test_listing: 'test list',
    compute_critical_path: 'compute',
    dynamic_analysis: 'dyn analysis',
    ensure_transitive_set_projection: 'tset',
  };
  const isOverhead = type === 'waiting' || type === 'generic_entry';
  return (
    <span
      className={`text-muted-foreground w-16 shrink-0 text-[10px] uppercase ${isOverhead ? 'italic' : ''}`}>
      {labels[type] ?? type}
    </span>
  );
}

/** JSON.stringify replacer that turns BigInts (and bigint-shaped objects from
 *  protobuf-ts/protobufjs Long) into plain strings so we don't crash on
 *  proto fields like start_offset_ns. */
function safeJsonReplacer(_key: string, value: unknown): unknown {
  if (typeof value === 'bigint') return value.toString();
  return value;
}

function EntryRow({
  entry,
  expanded,
  onToggle,
}: {
  entry: CriticalPathEntry;
  expanded: boolean;
  onToggle: () => void;
}) {
  const isWaiting = entry.type === 'waiting';

  return (
    <div>
      <button
        onClick={onToggle}
        className={`flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors hover:bg-gray-50 dark:hover:bg-gray-800 ${
          isWaiting ? 'text-muted-foreground' : ''
        } ${expanded ? 'bg-gray-50 dark:bg-gray-800' : ''}`}>
        {/* Start offset on top, +duration staggered down-and-right by half a
            line to read as "starts here, then continues for this much". */}
        <div className="w-20 shrink-0 flex flex-col items-end font-mono leading-none">
          <span className="pr-3 text-muted-foreground">
            {fmtSec(entry.startOffsetMs)}
          </span>
          <span
            className={`mt-1 text-[10px] ${
              isWaiting
                ? 'text-muted-foreground'
                : 'font-medium text-foreground/85'
            }`}>
            +{fmtSec(entry.wallDurationMs)}
          </span>
        </div>

        {/* Type */}
        <TypeBadge type={entry.type} />

        {/* Label */}
        <span className="min-w-0 flex-1 truncate font-mono" title={entry.label}>
          {entry.label}
        </span>

        {/* Execution kind for actions */}
        {entry.executionKind && (
          <ExecutionKindBadge kind={entry.executionKind} />
        )}
      </button>

      {/* Expanded view: pretty-printed JSON of the raw proto entry. Lets
          the user inspect the full data — durations, identifiers, kind-
          specific subfields — without us having to surface them by hand. */}
      {expanded && (
        <div className="ml-[5.75rem] border-l-2 border-gray-200 px-3 py-2 dark:border-gray-700">
          <pre className="overflow-auto rounded bg-amber-50 p-3 font-mono text-[11px] whitespace-pre-wrap break-all dark:bg-amber-950">
            {entry._raw
              ? JSON.stringify(entry._raw, safeJsonReplacer, 2)
              : '(raw entry not available — clear the log cache to re-decode)'}
          </pre>
        </div>
      )}
    </div>
  );
}

function PhaseSection({
  group,
  isFirst,
  isLast,
  expandedSet,
  onToggle,
  totalMs,
}: {
  group: PhaseGroup;
  isFirst: boolean;
  isLast: boolean;
  expandedSet: Set<number>;
  onToggle: (globalIdx: number) => void;
  totalMs: number;
}) {
  const [collapsed, setCollapsed] = useState(false);
  /** Incremented on each toggle to re-trigger the highlight animation */
  const [highlightCounter, setHighlightCounter] = useState(0);
  const sectionRef = useRef<HTMLDivElement>(null);
  const color = PHASE_COLORS[group.phase];
  const nonWaitEntries = group.entries.filter(e => e.type !== 'waiting');

  function handleToggleCollapsed() {
    // If the user is scrolled past the section's natural top (so the sticky
    // summary is currently pinned at the viewport top), keep that summary
    // at the viewport top after the toggle. Otherwise the section can
    // collapse entirely above the viewport and the user "loses" the phase
    // they were looking at.
    //
    // For the expand case (or when the section is in normal flow), the
    // section's natural top doesn't move, so no scroll adjustment is
    // needed — content below shifts but the section stays anchored.
    const el = sectionRef.current;
    const wasPinned = el ? el.getBoundingClientRect().top < 0 : false;
    setCollapsed(v => !v);
    setHighlightCounter(n => n + 1);
    if (wasPinned) {
      requestAnimationFrame(() => {
        sectionRef.current?.scrollIntoView({block: 'start'});
      });
    }
  }

  return (
    <div ref={sectionRef} className="relative flex items-start pl-[3px]">
      {/* Top-left curved cap + fading horizontal arm (first row only).
          Shifted -1px in both directions so the cap's 3px arc and 80px
          fade arm sit on top of the wrapper's 1px gray border, hiding
          it on the corner / left side and softly fading into it on the
          top. z-20 puts them above the sticky summary panel (z-10),
          which is opaque and would otherwise clip the cap/arm where
          they overlap with the panel's left edge. */}
      {isFirst && (
        <>
          <div
            className="pointer-events-none absolute z-20 h-2 w-2"
            style={{
              top: '-1px',
              left: '-1px',
              borderTop: `3px solid ${color}`,
              borderLeft: `3px solid ${color}`,
              borderTopLeftRadius: '8px',
            }}
          />
          <div
            className="pointer-events-none absolute z-20 h-[3px] w-20"
            style={{
              top: '-1px',
              left: '7px',
              background: `linear-gradient(to right, ${color}, transparent)`,
            }}
          />
        </>
      )}
      {/* Vertical phase bar — sits in the parent's pl-[3px] gutter so the
          sticky summary panel and entries area don't cover it. Shifted
          -1px to the left so it overlaps the wrapper's gray left border,
          and leaves 7px at the top/bottom of the first/last rows for the
          rounded caps to take over. */}
      <div
        className="pointer-events-none absolute w-[3px]"
        style={{
          backgroundColor: color,
          left: '-1px',
          top: isFirst ? '7px' : '0',
          bottom: isLast ? '7px' : '0',
        }}
      />
      {/* Bottom-left curved cap + fading horizontal arm (last row only).
          Same z-20 reason as the top cap above — for collapsed sections
          the sticky panel's bottom can reach the cap area. */}
      {isLast && (
        <>
          <div
            className="pointer-events-none absolute z-20 h-2 w-2"
            style={{
              bottom: '-1px',
              left: '-1px',
              borderBottom: `3px solid ${color}`,
              borderLeft: `3px solid ${color}`,
              borderBottomLeftRadius: '8px',
            }}
          />
          <div
            className="pointer-events-none absolute z-20 h-[3px] w-20"
            style={{
              bottom: '-1px',
              left: '7px',
              background: `linear-gradient(to right, ${color}, transparent)`,
            }}
          />
        </>
      )}

      {/* Highlight overlay — re-mounts on each toggle (key change)
          to retrigger the fade-out animation. Helps the user keep
          track of the section when scroll preservation can't put it
          back at the top of the viewport. */}
      {highlightCounter > 0 && (
        <div
          key={highlightCounter}
          className="phase-highlight-overlay pointer-events-none absolute inset-0 z-20"
          style={{backgroundColor: color}}
        />
      )}

      {/* Phase summary panel — sticky so it stays visible while scrolling
          through a long phase. self-start + sticky top-0 keeps the panel
          pinned to the top of the viewport while its containing section
          (the .flex parent) is in view. */}
      <div className="sticky top-0 z-10 w-[25%] shrink-0 self-start bg-background">
        <PhaseSummaryPanel
          group={group}
          totalMs={totalMs}
          headerAction={
            <button
              onClick={handleToggleCollapsed}
              className="text-muted-foreground hover:text-foreground shrink-0 rounded px-1.5 py-0.5 text-xs hover:bg-gray-100 dark:hover:bg-gray-800"
              title={collapsed ? 'Show entries' : 'Hide entries'}>
              {collapsed ? `▸ Show ${nonWaitEntries.length}` : '▾ Hide'}
            </button>
          }
        />
      </div>

      {/* Entries area: stub button when collapsed, otherwise the entries
          list. Animated via measured-height max-height transition so the
          animation works the same in both directions. The phase-colored
          left border lives here (not on the sticky panel) so the line
          spans the entire section height, not just the panel's. */}
      <div
        className="min-w-0 flex-1"
        style={{borderLeft: `1px solid ${color}`}}>
        {collapsed && (
          <button
            onClick={handleToggleCollapsed}
            className="text-muted-foreground hover:text-foreground w-full px-3 py-2 text-left text-xs italic hover:bg-gray-50 dark:hover:bg-gray-800">
            {nonWaitEntries.length} entries collapsed — click to expand
          </button>
        )}
        <CollapsibleEntries collapsed={collapsed}>
          {group.entries.map((entry, i) => {
            const globalIdx = (entry as unknown as {_globalIdx: number})
              ._globalIdx;
            return (
              <EntryRow
                key={i}
                entry={entry}
                expanded={expandedSet.has(globalIdx)}
                onToggle={() => onToggle(globalIdx)}
              />
            );
          })}
        </CollapsibleEntries>
      </div>
    </div>
  );
}

/**
 * Wraps content in an animated max-height container. Measures the content's
 * natural height with a ResizeObserver so the transition uses the exact
 * pixel value (animates correctly in both directions, regardless of how
 * many entries are inside).
 */
function CollapsibleEntries({
  collapsed,
  children,
}: {
  collapsed: boolean;
  children: React.ReactNode;
}) {
  const innerRef = useRef<HTMLDivElement>(null);
  // null until we've measured the natural height; before that, render with
  // no max-height constraint and no transition (so initial mount doesn't
  // animate from 0 to actual height).
  const [naturalHeight, setNaturalHeight] = useState<number | null>(null);

  useEffect(() => {
    const el = innerRef.current;
    if (!el) return;
    const measure = () => setNaturalHeight(el.scrollHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const measured = naturalHeight !== null;
  return (
    <div
      style={{
        maxHeight: measured ? (collapsed ? 0 : naturalHeight!) : undefined,
        overflow: 'hidden',
        transition: measured ? 'max-height 150ms ease-out' : undefined,
      }}>
      <div ref={innerRef}>{children}</div>
    </div>
  );
}

export default function CriticalPathView() {
  const logState = useEventLog();
  const [mode, setMode] = useUrlState<PathMode>('cp', 'slowest', {
    parse: s => (s === 'critical' ? 'critical' : 'slowest'),
  });
  const [expandedSet, setExpandedSet] = useState<Set<number>>(new Set());
  const [cpData, setCpData] = useState<CriticalPathData | null>(null);
  const [extractDone, setExtractDone] = useState(false);

  // Recompute whenever the event log state changes. The async extractor
  // ensures large-log buildGraphInfo chunks are loaded before extraction —
  // the sync version returns empty on LRU cache miss.
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
      <div className="text-muted-foreground flex items-center justify-center py-20">
        <p className="text-sm">
          {logState.status === 'loading' ? logState.progress : 'Loading...'}
        </p>
      </div>
    );
  }

  if (!extractDone) {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20">
        <p className="text-sm">Computing critical path...</p>
      </div>
    );
  }

  if (!cpData) {
    return (
      <div className="text-muted-foreground rounded border border-dashed p-12 text-center">
        <p className="text-lg font-medium">Critical Path</p>
        <p className="mt-1 text-sm">
          No critical path data found in event log.
        </p>
      </div>
    );
  }

  const groups = mode === 'critical' ? cpData.criticalPath : cpData.slowestPath;
  const totalMs =
    mode === 'critical' ? cpData.totalDurationMs : cpData.slowestTotalMs;

  // Tag entries with global indices for expansion tracking
  let globalIdx = 0;
  for (const group of groups) {
    for (const entry of group.entries) {
      (entry as unknown as {_globalIdx: number})._globalIdx = globalIdx++;
    }
  }

  function toggleExpanded(idx: number) {
    setExpandedSet(prev => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }

  return (
    <div>
      {/* Header */}
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-3">
          <PathToggle
            value={mode}
            onValueChange={m => {
              setMode(m);
              setExpandedSet(new Set());
            }}
          />
          <span className="text-muted-foreground text-xs">
            Total:{' '}
            <span className="font-medium text-foreground">
              {fmtSec(totalMs)}
            </span>
          </span>
        </div>

        {/* Phase legend */}
        <div className="flex gap-3">
          {groups.map(g => (
            <span key={g.phase} className="flex items-center gap-1 text-xs">
              <span
                className="inline-block h-2.5 w-2.5 rounded-sm"
                style={{backgroundColor: PHASE_COLORS[g.phase]}}
              />
              <span>{PHASE_LABELS[g.phase]}</span>
              <span className="text-muted-foreground">
                {fmtSec(g.totalDurationMs)}
              </span>
            </span>
          ))}
        </div>
      </div>

      {/* Phase groups — same C-shape phase outline + rounded gray frame
          as the summary view in CriticalPathChart. Sections touch each
          other (no space-y-2) so the per-phase colored bars form a
          continuous left edge; the color change at each boundary is the
          only separator. The wrapper's gray border is fully behind the
          colored elements: caps cover the rounded corners on the left,
          the vertical bar covers the straight left border, and the fade
          arms cover the top/bottom border where opaque (revealing the
          gray border again where the gradient goes transparent). */}
      <div className="relative rounded-lg border border-gray-400 dark:border-gray-500">
        {groups.map((group, i) => (
          <PhaseSection
            key={i}
            group={group}
            isFirst={i === 0}
            isLast={i === groups.length - 1}
            expandedSet={expandedSet}
            onToggle={toggleExpanded}
            totalMs={totalMs}
          />
        ))}
      </div>

      {/* Top-level targets */}
      {cpData.topLevelTargets.length > 1 && (
        <div className="mt-4">
          <h3 className="mb-2 text-sm font-semibold">Top-Level Targets</h3>
          <div className="space-y-1">
            {cpData.topLevelTargets.map((t, i) => (
              <div key={i} className="flex items-center gap-2 text-xs">
                <span className="w-16 shrink-0 text-right font-mono">
                  {fmtSec(t.durationMs)}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-1">
                    <div
                      className="h-2.5 rounded-sm bg-blue-500"
                      style={{
                        width: `${(t.durationMs / cpData.topLevelTargets[0].durationMs) * 100}%`,
                        minWidth: 2,
                      }}
                    />
                  </div>
                </div>
                <span
                  className="min-w-0 shrink truncate font-mono"
                  title={t.label}>
                  {t.label}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
