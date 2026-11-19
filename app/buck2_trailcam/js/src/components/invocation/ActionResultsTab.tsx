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

import {useEffect, useMemo, useRef, useState} from 'react';
import {useVirtualizer} from '@tanstack/react-virtual';
import {Badge, Input} from '../../ui';
import {useEventLog} from './EventLogProvider';
import {
  classifyStatus,
  executionKindLabel,
  flattenAction,
  type ActionDetail,
  type ActionStatus,
} from '../../lib/action-results';
import type {EventLogState} from './EventLogProvider';
import {formatBytes, formatDuration} from '../../lib/format';
import CopyButton from '../ui/CopyButton';
import {AnsiOutput, hasAnsiCodes} from '../ui/AnsiOutput';

type SortField = 'duration' | 'target' | 'status' | 'kind' | 'category';
type SortDir = 'asc' | 'desc';

const COMPACT_ROW_HEIGHT = 40;
// Columns: Status | Duration | Kind | Target | Category | Identifier
const GRID_TEMPLATE = '5rem 5rem 9rem minmax(0, 3fr) 9rem minmax(0, 2fr)';

interface ActionEntry {
  /** Index into the EventSummaryStore */
  storeIdx: number;
  status: ActionStatus;
  /** Raw executionKind enum string ("ACTION_EXECUTION_KIND_REMOTE", etc.) */
  kind: string;
  durationMs: number;
  target: string;
  /** First space-separated token of the actionName (stored as "category identifier"). */
  category: string;
  /** Everything after the first space — may contain spaces. */
  identifier: string;
  /** Lowercased haystack used for the search filter (built once). */
  searchText: string;
}

// Status presets: each preset is a `Set<ActionStatus>`. The "All" preset
// matches any status (we represent it as an empty set in selectedStatuses).
// `activeClass` is the muted-tint background applied when the preset is the
// currently selected status set, picked to match the status colors used
// elsewhere (badge, pass/fail bar) so the active chip semantically reflects
// what's filtered.
type StatusPresetId = 'all' | 'failed' | 'ran' | 'cached';
const STATUS_PRESETS: ReadonlyArray<{
  id: StatusPresetId;
  label: string;
  statuses: ReadonlySet<ActionStatus>;
  activeClass: string;
}> = [
  {
    id: 'all',
    label: 'All',
    statuses: new Set<ActionStatus>(),
    activeClass: 'bg-gray-100 text-foreground dark:bg-gray-800',
  },
  {
    id: 'failed',
    label: 'Failed',
    statuses: new Set<ActionStatus>(['failed']),
    activeClass: 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300',
  },
  {
    id: 'ran',
    label: 'Ran',
    statuses: new Set<ActionStatus>(['success', 'failed', 'unknown']),
    activeClass:
      'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300',
  },
  {
    id: 'cached',
    label: 'Cached',
    statuses: new Set<ActionStatus>(['cached']),
    activeClass:
      'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300',
  },
];

function setsEqual<T>(a: ReadonlySet<T>, b: ReadonlySet<T>): boolean {
  if (a.size !== b.size) return false;
  for (const v of a) if (!b.has(v)) return false;
  return true;
}

function statusBadgeClass(s: ActionStatus): string {
  switch (s) {
    case 'success':
      return 'bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300';
    case 'failed':
      return 'bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300';
    case 'cached':
      return 'bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300';
    case 'unknown':
      return 'bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400';
  }
}

export default function ActionResultsTab() {
  const logState = useEventLog();

  if (logState.status === 'idle' || logState.status === 'loading') {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20 text-sm">
        {logState.status === 'loading' ? logState.progress : 'Loading...'}
      </div>
    );
  }
  if (logState.status === 'error') {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20 text-sm">
        No event log available.
      </div>
    );
  }

  return <ActionsView logState={logState} />;
}

function ActionsView({
  logState,
}: {
  logState: Extract<EventLogState, {status: 'loaded'}>;
}) {
  // Walk the event log once via fast columnar accessors.
  const entries = useMemo(() => {
    const summaries = logState.summaries;
    const out: ActionEntry[] = [];
    for (let i = 0; i < summaries.length; i++) {
      if (summaries.getEventType(i) !== 'actionExecution') continue;
      if (summaries.getType(i) !== 'spanEnd') continue;
      const target = summaries.getTargetLabel(i) ?? '';
      const actionName = summaries.getActionName(i) ?? '';
      const kind = summaries.getExecutionKind(i) ?? '';
      // actionName is "category identifier" joined with a space (or just
      // "category" when there's no identifier). Split on first space.
      const sp = actionName.indexOf(' ');
      const category = sp >= 0 ? actionName.slice(0, sp) : actionName;
      const identifier = sp >= 0 ? actionName.slice(sp + 1) : '';
      out.push({
        storeIdx: i,
        status: classifyStatus(summaries.getFailed(i), kind),
        kind,
        durationMs: summaries.getDurationMs(i) ?? 0,
        target,
        category,
        identifier,
        searchText: `${target} ${category} ${identifier} ${kind}`.toLowerCase(),
      });
    }
    return out;
  }, [logState.summaries]);

  const counts = useMemo(() => {
    const c = {
      all: entries.length,
      success: 0,
      failed: 0,
      cached: 0,
      unknown: 0,
    };
    for (const e of entries) c[e.status]++;
    return c;
  }, [entries]);

  const categoryOptions = useMemo(() => {
    const m = new Map<string, number>();
    for (const e of entries) m.set(e.category, (m.get(e.category) ?? 0) + 1);
    return Array.from(m.entries()).sort((a, b) => b[1] - a[1]);
  }, [entries]);

  const kindOptions = useMemo(() => {
    const m = new Map<string, number>();
    for (const e of entries) m.set(e.kind, (m.get(e.kind) ?? 0) + 1);
    return Array.from(m.entries()).sort((a, b) => b[1] - a[1]);
  }, [entries]);

  // Default selection: failed only if any failures, else "ran" (i.e., status
  // ∈ {success, failed, unknown} — excludes cache hits).
  const [selectedStatuses, setSelectedStatuses] = useState<Set<ActionStatus>>(
    () =>
      counts.failed > 0
        ? new Set<ActionStatus>(['failed'])
        : new Set<ActionStatus>(['success', 'failed', 'unknown']),
  );
  const [selectedKinds, setSelectedKinds] = useState<Set<string>>(
    () => new Set(),
  );
  const [selectedCategories, setSelectedCategories] = useState<Set<string>>(
    () => new Set(),
  );
  const [search, setSearch] = useState('');
  const [sortField, setSortField] = useState<SortField>('duration');
  const [sortDir, setSortDir] = useState<SortDir>('desc');
  const [expanded, setExpanded] = useState<Set<number>>(() => new Set());

  const filtered = useMemo(() => {
    const lower = search.toLowerCase();
    const out = entries.filter(e => {
      if (selectedStatuses.size > 0 && !selectedStatuses.has(e.status))
        return false;
      if (selectedKinds.size > 0 && !selectedKinds.has(e.kind)) return false;
      if (selectedCategories.size > 0 && !selectedCategories.has(e.category))
        return false;
      if (search && !e.searchText.includes(lower)) return false;
      return true;
    });
    out.sort((a, b) => {
      const dir = sortDir === 'asc' ? 1 : -1;
      let av: string | number;
      let bv: string | number;
      switch (sortField) {
        case 'duration':
          av = a.durationMs;
          bv = b.durationMs;
          break;
        case 'target':
          av = a.target || a.category;
          bv = b.target || b.category;
          break;
        case 'status':
          av = a.status;
          bv = b.status;
          break;
        case 'kind':
          av = a.kind;
          bv = b.kind;
          break;
        case 'category':
          av = a.category;
          bv = b.category;
          break;
      }
      if (av < bv) return -1 * dir;
      if (av > bv) return 1 * dir;
      return a.storeIdx - b.storeIdx;
    });
    return out;
  }, [
    entries,
    selectedStatuses,
    selectedKinds,
    selectedCategories,
    search,
    sortField,
    sortDir,
  ]);

  const toggleExpand = (idx: number) =>
    setExpanded(prev => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });

  const toggleSort = (field: SortField) => {
    if (sortField === field) {
      setSortDir(d => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortField(field);
      // Sensible per-field defaults: duration desc (slow first), others asc.
      setSortDir(field === 'duration' ? 'desc' : 'asc');
    }
  };

  // Counts for each status preset, used to label the chips and hide ones
  // with zero matches. "all" is always present; the others are hidden when
  // they'd select nothing.
  const presetCount = (id: StatusPresetId): number => {
    switch (id) {
      case 'all':
        return counts.all;
      case 'failed':
        return counts.failed;
      case 'ran':
        return counts.success + counts.failed + counts.unknown;
      case 'cached':
        return counts.cached;
    }
  };

  // Which preset (if any) currently matches the selectedStatuses set?
  const activePreset = STATUS_PRESETS.find(p =>
    setsEqual(p.statuses, selectedStatuses),
  )?.id;

  if (entries.length === 0) {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20 text-sm">
        No actions in this build.
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* Header */}
      <div className="mb-4 flex items-center justify-between">
        <div className="flex items-center gap-4">
          <span className="text-lg font-semibold">{counts.all} actions</span>
        </div>
        <div className="flex gap-3 text-sm">
          {counts.success > 0 && (
            <span className="text-green-600 dark:text-green-400">
              {counts.success} success
            </span>
          )}
          {counts.failed > 0 && (
            <span className="font-medium text-red-600 dark:text-red-400">
              {counts.failed} failed
            </span>
          )}
          {counts.cached > 0 && (
            <span className="text-blue-600 dark:text-blue-400">
              {counts.cached} cached
            </span>
          )}
          {counts.unknown > 0 && (
            <span className="text-muted-foreground">
              {counts.unknown} other
            </span>
          )}
        </div>
      </div>

      {/* Status bar */}
      {counts.all > 0 && (
        <div className="mb-4 flex h-2 w-full overflow-hidden rounded-full bg-gray-100 dark:bg-gray-800">
          {counts.success > 0 && (
            <div
              className="bg-green-500"
              style={{width: `${(counts.success / counts.all) * 100}%`}}
            />
          )}
          {counts.failed > 0 && (
            <div
              className="bg-red-500"
              style={{width: `${(counts.failed / counts.all) * 100}%`}}
            />
          )}
          {counts.cached > 0 && (
            <div
              className="bg-blue-500"
              style={{width: `${(counts.cached / counts.all) * 100}%`}}
            />
          )}
          {counts.unknown > 0 && (
            <div
              className="bg-gray-400"
              style={{width: `${(counts.unknown / counts.all) * 100}%`}}
            />
          )}
        </div>
      )}

      {/* Toolbar: status presets (shortcuts) + multi-select filters + search */}
      <div className="mb-3 flex flex-wrap items-center gap-3">
        <div className="flex gap-1">
          {STATUS_PRESETS.map(p => {
            const count = presetCount(p.id);
            if (count === 0 && p.id !== 'all') return null;
            const isActive = activePreset === p.id;
            return (
              <button
                key={p.id}
                onClick={() => setSelectedStatuses(new Set(p.statuses))}
                className={`rounded px-2 py-0.5 text-xs font-medium transition-colors ${
                  isActive
                    ? p.activeClass
                    : 'text-muted-foreground font-normal hover:bg-gray-100 dark:hover:bg-gray-800'
                }`}>
                {p.label} ({count})
              </button>
            );
          })}
        </div>

        <MultiSelectFilter
          label="Category"
          options={categoryOptions}
          selected={selectedCategories}
          onChange={setSelectedCategories}
        />
        <MultiSelectFilter
          label="Execution kind"
          options={kindOptions}
          selected={selectedKinds}
          onChange={setSelectedKinds}
          formatOption={executionKindLabel}
        />

        <Input
          type="text"
          placeholder="Search target / category / identifier..."
          value={search}
          onChange={e => setSearch(e.target.value)}
          className="ml-auto h-7 max-w-xs text-xs"
        />
      </div>

      {/* Table — virtualized rows; sticky header outside the scroll area. */}
      <div className="flex min-h-0 flex-1 flex-col rounded border">
        <div
          className="grid items-center gap-x-2 border-b bg-gray-50 px-2 py-1.5 text-xs font-medium dark:bg-gray-900"
          style={{gridTemplateColumns: GRID_TEMPLATE}}>
          <SortHeader
            field="status"
            label="Status"
            sortField={sortField}
            sortDir={sortDir}
            onClick={toggleSort}
          />
          <SortHeader
            field="duration"
            label="Duration"
            sortField={sortField}
            sortDir={sortDir}
            onClick={toggleSort}
          />
          <SortHeader
            field="kind"
            label="Kind"
            sortField={sortField}
            sortDir={sortDir}
            onClick={toggleSort}
          />
          <SortHeader
            field="target"
            label="Target"
            sortField={sortField}
            sortDir={sortDir}
            onClick={toggleSort}
          />
          <SortHeader
            field="category"
            label="Category"
            sortField={sortField}
            sortDir={sortDir}
            onClick={toggleSort}
          />
          <span>Identifier</span>
        </div>

        <VirtualBody
          filtered={filtered}
          expanded={expanded}
          onToggle={toggleExpand}
          logState={logState}
        />
      </div>

      {filtered.length === 0 && entries.length > 0 && (
        <p className="text-muted-foreground py-4 text-center text-xs">
          No actions match the current filters.
        </p>
      )}
    </div>
  );
}

function VirtualBody({
  filtered,
  expanded,
  onToggle,
  logState,
}: {
  filtered: ActionEntry[];
  expanded: Set<number>;
  onToggle: (idx: number) => void;
  logState: Extract<EventLogState, {status: 'loaded'}>;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => COMPACT_ROW_HEIGHT,
    overscan: 20,
  });

  return (
    <div ref={parentRef} className="min-h-0 flex-1 overflow-auto">
      <div
        style={{
          height: `${virtualizer.getTotalSize()}px`,
          position: 'relative',
        }}>
        {virtualizer.getVirtualItems().map(vRow => {
          const entry = filtered[vRow.index];
          const isExpanded = expanded.has(entry.storeIdx);
          return (
            <div
              key={entry.storeIdx}
              data-index={vRow.index}
              // Only measure expanded rows — collapsed are exactly COMPACT_ROW_HEIGHT.
              ref={isExpanded ? virtualizer.measureElement : undefined}
              className="absolute left-0 w-full"
              style={{transform: `translateY(${vRow.start}px)`}}>
              <CompactRow
                entry={entry}
                expanded={isExpanded}
                onToggle={() => onToggle(entry.storeIdx)}
              />
              {isExpanded && (
                <ExpandedRow storeIdx={entry.storeIdx} logState={logState} />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function CompactRow({
  entry,
  expanded,
  onToggle,
}: {
  entry: ActionEntry;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <div
      onClick={onToggle}
      className={`grid cursor-pointer items-center gap-x-2 border-b border-gray-100 px-2 py-1.5 text-sm transition-colors hover:bg-gray-50 dark:border-gray-800 dark:hover:bg-gray-800 ${
        expanded ? 'bg-gray-50 dark:bg-gray-900' : ''
      }`}
      style={{
        gridTemplateColumns: GRID_TEMPLATE,
        minHeight: COMPACT_ROW_HEIGHT,
      }}>
      <div>
        <Badge
          className={`text-[10px] px-1 py-0 ${statusBadgeClass(entry.status)}`}>
          {entry.status}
        </Badge>
      </div>
      <span className="font-mono">
        {entry.durationMs > 0 ? formatDuration(entry.durationMs) : '—'}
      </span>
      <span className="text-muted-foreground truncate" title={entry.kind}>
        {executionKindLabel(entry.kind)}
      </span>
      <span className="truncate font-mono" title={entry.target}>
        {entry.target || ''}
      </span>
      <span className="truncate font-mono" title={entry.category}>
        {entry.category || ''}
      </span>
      <span
        className="text-muted-foreground truncate font-mono"
        title={entry.identifier}>
        {entry.identifier || ''}
      </span>
    </div>
  );
}

function SortHeader({
  field,
  label,
  sortField,
  sortDir,
  onClick,
}: {
  field: SortField;
  label: string;
  sortField: SortField;
  sortDir: SortDir;
  onClick: (field: SortField) => void;
}) {
  return (
    <span
      onClick={() => onClick(field)}
      className="cursor-pointer select-none truncate hover:text-foreground">
      {label}
      {sortField === field && (sortDir === 'asc' ? ' ↑' : ' ↓')}
    </span>
  );
}

function MultiSelectFilter({
  label,
  options,
  selected,
  onChange,
  formatOption,
}: {
  label: string;
  options: ReadonlyArray<readonly [string, number]>;
  selected: Set<string>;
  onChange: (next: Set<string>) => void;
  formatOption?: (value: string) => string;
}) {
  const [open, setOpen] = useState(false);
  const display = formatOption ?? ((v: string) => v);
  return (
    <div className="relative">
      <button
        onClick={() => setOpen(!open)}
        className="flex items-center gap-1.5 rounded border px-2 py-1 text-xs hover:bg-gray-50 dark:hover:bg-gray-800">
        <span>{label}</span>
        {selected.size > 0 && (
          <Badge variant="secondary" className="px-1 py-0 text-[10px]">
            {selected.size}
          </Badge>
        )}
        <span className="text-muted-foreground text-[10px]">▾</span>
      </button>
      {open && (
        <>
          <div className="fixed inset-0 z-20" onClick={() => setOpen(false)} />
          <div className="absolute left-0 z-30 mt-1 max-h-72 w-72 overflow-auto rounded border bg-white shadow-lg dark:bg-gray-900">
            {selected.size > 0 && (
              <button
                onClick={() => onChange(new Set())}
                className="text-muted-foreground block w-full border-b border-gray-100 px-3 py-1.5 text-left text-xs hover:bg-gray-50 dark:border-gray-800 dark:hover:bg-gray-800">
                Clear ({selected.size})
              </button>
            )}
            {options.length === 0 && (
              <div className="text-muted-foreground px-3 py-2 text-xs">
                No values.
              </div>
            )}
            {options.map(([value, count]) => {
              const isSel = selected.has(value);
              return (
                <button
                  key={value || '__empty__'}
                  onClick={() => {
                    const next = new Set(selected);
                    if (isSel) next.delete(value);
                    else next.add(value);
                    onChange(next);
                  }}
                  className="flex w-full items-center justify-between gap-2 px-3 py-1.5 text-xs hover:bg-gray-50 dark:hover:bg-gray-800">
                  <span className="flex min-w-0 items-center gap-2">
                    <input
                      type="checkbox"
                      checked={isSel}
                      readOnly
                      className="shrink-0"
                    />
                    <span className="truncate font-mono">
                      {value ? display(value) : '(none)'}
                    </span>
                  </span>
                  <Badge
                    variant="secondary"
                    className="shrink-0 px-1 py-0 text-[10px]">
                    {count}
                  </Badge>
                </button>
              );
            })}
          </div>
        </>
      )}
    </div>
  );
}

function ExpandedRow({
  storeIdx,
  logState,
}: {
  storeIdx: number;
  logState: Extract<EventLogState, {status: 'loaded'}>;
}) {
  const [detail, setDetail] = useState<ActionDetail | null>(null);

  useEffect(() => {
    let cancelled = false;
    const summary = logState.summaries.get(storeIdx);
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
  }, [logState, storeIdx]);

  if (!detail) {
    return (
      <div className="bg-gray-50 px-4 py-3 text-xs text-muted-foreground dark:bg-gray-900">
        Loading details...
      </div>
    );
  }

  return (
    <div className="space-y-3 bg-gray-50 px-4 py-3 text-xs dark:bg-gray-900">
      {/* Metadata */}
      <div className="grid gap-x-4 gap-y-1 sm:grid-cols-2">
        <MetaRow label="Target">
          <span className="font-mono break-all">
            {detail.configuredTargetLabel ?? '—'}
          </span>
        </MetaRow>
        {(detail.category || detail.identifier) && (
          <MetaRow label="Action">
            <span className="font-mono">
              {[detail.category, detail.identifier].filter(Boolean).join(' ')}
            </span>
          </MetaRow>
        )}
        <MetaRow label="Kind">
          <span>{executionKindLabel(detail.executionKind ?? undefined)}</span>
          {detail.executionKind &&
            detail.executionKind !==
              executionKindLabel(detail.executionKind) && (
              <span className="text-muted-foreground ml-1">
                ({detail.executionKind})
              </span>
            )}
        </MetaRow>
        {detail.exitCode != null && detail.exitCode !== 0 && (
          <MetaRow label="Exit code">
            <span className="font-mono">{detail.exitCode}</span>
          </MetaRow>
        )}
        {detail.wallTimeMs != null && detail.wallTimeMs > 0 && (
          <MetaRow label="Wall time">
            {formatDuration(detail.wallTimeMs)}
          </MetaRow>
        )}
        {detail.outputSizeBytes != null && detail.outputSizeBytes > 0 && (
          <MetaRow label="Output size">
            {formatBytes(detail.outputSizeBytes)}
          </MetaRow>
        )}
        {detail.hostname && (
          <MetaRow label="Hostname">
            <span className="font-mono">{detail.hostname}</span>
          </MetaRow>
        )}
        {detail.actionDigest && (
          <MetaRow label="Action digest">
            <span className="break-all font-mono">{detail.actionDigest}</span>
          </MetaRow>
        )}
      </div>

      {detail.repro.kind === 'shell' && (
        <CodeSection title="Repro" body={detail.repro.command} copy />
      )}
      {detail.additionalMessage && (
        <CodeSection title="Buck message" body={detail.additionalMessage} />
      )}
      {detail.stdout && (
        <CodeSection title="Stdout" body={detail.stdout} maxHeight="max-h-56" />
      )}
      {detail.stderr && (
        <CodeSection title="Stderr" body={detail.stderr} maxHeight="max-h-56" />
      )}
    </div>
  );
}

function CodeSection({
  title,
  body,
  copy = false,
  maxHeight = 'max-h-40',
}: {
  title: string;
  body: string;
  copy?: boolean;
  maxHeight?: string;
}) {
  const ansi = hasAnsiCodes(body);
  // Default to rendered when ANSI is detected; the user can flip to raw to
  // see the literal escape sequences (useful for copy-pasting or debugging
  // output that isn't rendering quite right).
  const [raw, setRaw] = useState(false);
  return (
    <div>
      <div className="mb-1 flex items-center gap-2">
        <span className="font-medium">{title}</span>
        {copy && <CopyButton text={body} label="Copy" />}
        {ansi && (
          <button
            onClick={() => setRaw(r => !r)}
            className="text-muted-foreground hover:text-foreground rounded px-1 text-[11px] hover:bg-gray-100 dark:hover:bg-gray-700"
            title={
              raw
                ? 'Show with ANSI styling applied'
                : 'Show raw escape sequences'
            }>
            {raw ? 'Rendered' : 'Raw'}
          </button>
        )}
      </div>
      <pre
        className={`${maxHeight} overflow-auto rounded bg-amber-50 p-2 font-mono text-sm whitespace-pre-wrap dark:bg-amber-950`}>
        {ansi && !raw ? <AnsiOutput>{body}</AnsiOutput> : body}
      </pre>
    </div>
  );
}

function MetaRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-baseline gap-2">
      <span className="text-muted-foreground shrink-0">{label}:</span>
      <span className="min-w-0">{children}</span>
    </div>
  );
}
