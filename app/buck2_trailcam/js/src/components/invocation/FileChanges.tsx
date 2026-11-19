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

import {useState, useEffect, useMemo} from 'react';
import {Badge, Card, CardContent, CardHeader, CardTitle, Input} from '../../ui';
import {
  useBackend,
  type DaemonFileChangesResponse,
  type FileChangeEntry,
} from '../../backend';

type LoadState =
  | {status: 'idle'}
  | {status: 'unavailable'}
  | {status: 'loading'}
  | {status: 'error'; message: string}
  | {status: 'loaded'; data: DaemonFileChangesResponse};

/** Default-visible row count per section. Lists shorter than this never
 *  show a collapse toggle. */
const COLLAPSE_THRESHOLD = 30;

/** Color tokens per section, keyed off `color`. Mirrors the daemon /
 *  sandcastle / rebase boundary marker palettes from BuildHistorySidebar. */
const SECTION_COLORS: Record<
  'indigo' | 'emerald',
  {text: string; line: string}
> = {
  indigo: {
    text: 'text-indigo-700 dark:text-indigo-300',
    line: 'bg-indigo-300 dark:bg-indigo-800',
  },
  emerald: {
    text: 'text-emerald-700 dark:text-emerald-300',
    line: 'bg-emerald-300 dark:bg-emerald-800',
  },
};

/**
 * Parse a file change entry like "1:0:path/to/file" into structured form.
 * Falls back to treating the input as a plain path if it doesn't match.
 */
function parseEntry(entry: string): FileChangeEntry {
  const firstColon = entry.indexOf(':');
  if (firstColon < 0) {
    return {eventType: '', fileType: '', path: entry};
  }
  const secondColon = entry.indexOf(':', firstColon + 1);
  if (secondColon < 0) {
    return {eventType: '', fileType: '', path: entry};
  }
  return {
    eventType: entry.slice(0, firstColon),
    fileType: entry.slice(firstColon + 1, secondColon),
    path: entry.slice(secondColon + 1),
  };
}

export default function FileChanges({
  uuid,
  buildStartTimeMs,
  changesSinceLastBuild,
  changesSinceLastBuildCount,
}: {
  uuid: string;
  /** Wall-clock start time of the invocation (unix ms). Lets the host centre
   *  its daemon-session lookup on the actual build instead of "now". */
  buildStartTimeMs?: number | null;
  /** Raw `<event>:<type>:<path>` entries changed since the previous build on
   *  this daemon (see `InvocationMetrics`); may be truncated. */
  changesSinceLastBuild: readonly string[];
  /** Full count behind `changesSinceLastBuild`, or null if unknown. */
  changesSinceLastBuildCount: number | null;
}) {
  const {queryDaemonFileChanges} = useBackend();
  const [state, setState] = useState<LoadState>({status: 'idle'});
  const [search, setSearch] = useState('');

  // Section 1: changes from the most recent buck command (this build only).
  const sinceLastBuild = useMemo(
    () => changesSinceLastBuild.map(e => parseEntry(e)),
    [changesSinceLastBuild],
  );
  const sinceLastBuildTotal =
    changesSinceLastBuildCount ?? sinceLastBuild.length;

  // Section 2: accumulated changes since the daemon's branched-from revision,
  // deduplicated against section 1 so we only show the *additional* ones.
  useEffect(() => {
    const query = queryDaemonFileChanges;
    if (!query) {
      setState({status: 'unavailable'});
      return;
    }
    let cancelled = false;
    setState({status: 'loading'});
    query(uuid, buildStartTimeMs ?? null).then(
      (data: DaemonFileChangesResponse) => {
        if (!cancelled) setState({status: 'loaded', data});
      },
      (e: unknown) => {
        if (!cancelled) {
          setState({
            status: 'error',
            message: e instanceof Error ? e.message : 'Failed to load',
          });
        }
      },
    );
    return () => {
      cancelled = true;
    };
  }, [uuid, buildStartTimeMs, queryDaemonFileChanges]);

  const additionalSinceMergebase = useMemo(() => {
    if (state.status !== 'loaded') return [];
    const lastBuildPaths = new Set(sinceLastBuild.map(e => e.path));
    return state.data.changes.filter(e => !lastBuildPaths.has(e.path));
  }, [state, sinceLastBuild]);

  // Show the shared filter once either section's loaded entries push the
  // total above the per-section collapse threshold — i.e. once filtering
  // could plausibly help.
  const totalLoaded = sinceLastBuild.length + additionalSinceMergebase.length;
  const showFilter = totalLoaded > COLLAPSE_THRESHOLD;

  // When the user has typed something but neither section has any matches,
  // surface a single "No matches" message instead of just hiding both
  // sections silently.
  const lower = search.toLowerCase();
  const lastBuildHasMatch =
    !search || sinceLastBuild.some(e => e.path.toLowerCase().includes(lower));
  const mergebaseHasMatch =
    !search ||
    additionalSinceMergebase.some(e => e.path.toLowerCase().includes(lower));
  const noMatches = !!search && !lastBuildHasMatch && !mergebaseHasMatch;

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base">Files changed</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        {showFilter && (
          <Input
            type="text"
            placeholder="Filter file paths…"
            value={search}
            onChange={e => setSearch(e.target.value)}
            className="h-7 text-xs"
          />
        )}
        <FileChangesSection
          title="Since last buck command"
          entries={sinceLastBuild}
          totalCount={sinceLastBuildTotal}
          emptyMessage="No changes since last buck command"
          search={search}
          color="indigo"
        />
        {state.status !== 'unavailable' && (
          <FileChangesSection
            title="Additional changes since mergebase"
            entries={additionalSinceMergebase}
            totalCount={
              state.status === 'loaded' ? additionalSinceMergebase.length : null
            }
            loading={state.status === 'loading' || state.status === 'idle'}
            error={state.status === 'error' ? state.message : null}
            subtitle={
              state.status === 'loaded'
                ? `Across ${state.data.buildCount} build${state.data.buildCount !== 1 ? 's' : ''} since branch point`
                : null
            }
            emptyMessage="No additional changes since mergebase"
            search={search}
            color="emerald"
          />
        )}
        {noMatches && (
          <p className="text-muted-foreground text-center text-xs">
            No matches
          </p>
        )}
      </CardContent>
    </Card>
  );
}

function FileChangesSection({
  title,
  entries,
  totalCount,
  loading = false,
  error = null,
  subtitle = null,
  emptyMessage,
  search,
  color,
}: {
  title: string;
  entries: FileChangeEntry[];
  totalCount: number | null;
  loading?: boolean;
  error?: string | null;
  subtitle?: string | null;
  emptyMessage: string;
  search: string;
  color: keyof typeof SECTION_COLORS;
}) {
  const [expanded, setExpanded] = useState(false);

  const showCount = totalCount ?? entries.length;

  const filtered = useMemo(() => {
    if (!search) return entries;
    const lower = search.toLowerCase();
    return entries.filter(e => e.path.toLowerCase().includes(lower));
  }, [entries, search]);

  // When the shared filter is active and this section has no matches, hide
  // the whole section. The parent surfaces a single "No matches" message
  // when both sections come up empty.
  if (search && !loading && !error && filtered.length === 0) return null;

  const overflowing = !search && filtered.length > COLLAPSE_THRESHOLD;
  const visible =
    expanded || !overflowing ? filtered : filtered.slice(0, COLLAPSE_THRESHOLD);
  const hidden =
    overflowing && !expanded ? filtered.length - COLLAPSE_THRESHOLD : 0;

  const c = SECTION_COLORS[color];

  return (
    <section className="relative">
      {/* Title bar — fixed 24px height so we can position the vertical line
          deterministically at its center. With `items-center`, the 2px-tall
          line stubs sit at y=12 (row's vertical middle), passing through the
          center of the title text and badge so the title appears to sit on
          the line. */}
      <div
        className={`flex h-6 items-center gap-2 text-[11px] uppercase tracking-wide ${c.text}`}>
        <div className={`h-0.5 w-2 shrink-0 ${c.line}`} />
        <span className="font-medium tracking-wide">{title}</span>
        {!loading && !error && showCount > 0 && (
          <Badge variant="secondary" className="px-1 py-0 text-[10px]">
            {showCount}
          </Badge>
        )}
        <div className={`h-0.5 flex-1 ${c.line}`} />
      </div>

      {/* Vertical line — absolute relative to the section. Its top is at
          y=12 (= half of the title row's h-6) so it lands exactly on the
          horizontal line stub's center, forming a continuous L corner. */}
      <div className={`absolute top-3 bottom-0 left-0 w-0.5 ${c.line}`} />

      <div className="pl-3 pb-1">
        {subtitle && (
          <p className="text-muted-foreground mt-0.5 text-[11px]">{subtitle}</p>
        )}
        {loading && (
          <p className="text-muted-foreground mt-1 text-xs">Loading…</p>
        )}
        {error && (
          <p className="mt-1 text-xs text-red-600 dark:text-red-400">{error}</p>
        )}
        {!loading && !error && entries.length === 0 && (
          <p className="text-muted-foreground mt-1 text-xs italic">
            {emptyMessage}
          </p>
        )}
        {!loading && !error && entries.length > 0 && (
          <>
            <ul className="mt-1 space-y-0.5">
              {visible.map((change, i) => (
                <li
                  key={`${change.path}-${i}`}
                  className="flex items-center gap-1.5 truncate font-mono text-xs"
                  title={change.path}>
                  <EventTypeBadge type={change.eventType} />
                  <span className="truncate">{change.path}</span>
                </li>
              ))}
            </ul>
            {hidden > 0 && (
              <button
                onClick={() => setExpanded(true)}
                className="text-muted-foreground hover:text-foreground mt-1 text-xs hover:underline">
                Show {hidden} more file{hidden === 1 ? '' : 's'}
              </button>
            )}
            {expanded && overflowing && (
              <button
                onClick={() => setExpanded(false)}
                className="text-muted-foreground hover:text-foreground mt-1 text-xs hover:underline">
                Show less
              </button>
            )}
          </>
        )}
      </div>
    </section>
  );
}

function EventTypeBadge({type}: {type: string}) {
  // Event type flags from buck2: https://fburl.com/code/yidgbakn
  // Common values: "c" = create, "m" = modify, "d" = delete
  const label =
    type === 'c' || type === '0'
      ? 'C'
      : type === 'm' || type === '1'
        ? 'M'
        : type === 'd' || type === '2'
          ? 'D'
          : type;

  const color =
    label === 'C'
      ? 'text-green-600 dark:text-green-400'
      : label === 'M'
        ? 'text-yellow-600 dark:text-yellow-400'
        : label === 'D'
          ? 'text-red-600 dark:text-red-400'
          : 'text-gray-500';

  return (
    <span className={`w-3 shrink-0 text-center text-[10px] font-bold ${color}`}>
      {label}
    </span>
  );
}
