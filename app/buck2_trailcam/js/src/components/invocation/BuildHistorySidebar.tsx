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

import {useState, useEffect, useCallback, useRef} from 'react';
import {Badge} from '../../ui';
import {formatBytes, formatDuration} from '../../lib/format';
import {useBackend, type BuildHistoryRow} from '../../backend';

interface BuildHistorySidebarProps {
  currentUuid: string;
  username: string | null;
  hostname: string | null;
  isolationDir: string | null;
  repository: string | null;
  // Start time (epoch ms) of the build the user is viewing. Used to seed the
  // first Scuba query around that build's time rather than around "now". Null
  // until the invocation info has loaded.
  currentBuildStartMs: number | null;
  onNavigate: (uuid: string) => void;
}

function outcomeBorderColor(outcome: string): string {
  switch (outcome) {
    case 'SUCCESS':
      return 'border-l-green-500';
    case 'FAILED':
      return 'border-l-red-500';
    case 'RUNNING':
      return 'border-l-blue-500';
    default:
      return 'border-l-gray-300 dark:border-l-gray-600';
  }
}

function formatAbsoluteTime(epochMs: number): string {
  const d = new Date(epochMs);
  const now = new Date();
  const time = d.toLocaleTimeString([], {hour: '2-digit', minute: '2-digit'});
  if (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  ) {
    return time;
  }
  if (d.getFullYear() === now.getFullYear()) {
    return `${d.toLocaleDateString([], {month: 'short', day: 'numeric'})} ${time}`;
  }
  return `${d.toLocaleDateString([], {month: 'short', day: 'numeric', year: 'numeric'})} ${time}`;
}

export default function BuildHistorySidebar({
  currentUuid,
  username,
  hostname,
  isolationDir,
  repository,
  currentBuildStartMs,
  onNavigate,
}: BuildHistorySidebarProps) {
  const {queryBuildHistory} = useBackend();
  const [allBuilds, setAllBuilds] = useState<BuildHistoryRow[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasMore, setHasMore] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Bounds of the time range we've actually queried Scuba for. Newest is the
  // very first fetch's endTime; oldest is the most recent fetch's endTime
  // minus the batch window. Used to show coverage markers in the sidebar.
  const [searchedNewestMs, setSearchedNewestMs] = useState<number | null>(null);
  const [searchedOldestMs, setSearchedOldestMs] = useState<number | null>(null);
  // Pagination cursor: the window we are currently exploring. cursorEndMs
  // shrinks toward windowStartMs as we drain a cap-saturated window across
  // multiple fetches; once a fetch returns less than the cap we move to a
  // brand-new window (windowStartMs - windowSizeMsRef.current, windowStartMs).
  const explorationRef = useRef<{
    windowStartMs: number;
    cursorEndMs: number;
    capHits: number;
  } | null>(null);
  // True once pagination has walked back past the look-back floor and there's
  // nothing older in scope. (We also use this to communicate "stop" to the
  // bottom marker via reachedFloor.)
  const [reachedFloor, setReachedFloor] = useState(false);
  // Time ranges where we hit the row cap so many times in one window that we
  // gave up draining it. Surfaced inline so the user knows builds in those
  // ranges may be missing.
  const [incompleteRanges, setIncompleteRanges] = useState<
    Array<{startMs: number; endMs: number}>
  >([]);
  const sentinelRef = useRef<HTMLDivElement>(null);
  const lookbackMs = 30 * 24 * 60 * 60 * 1000; // 30 days
  const FETCH_LIMIT = 200;
  const MAX_CAP_HITS_PER_WINDOW = 5;
  // Adaptive window sizing: aim each batch at ~TARGET_ROWS_PER_WINDOW
  // returned rows. Tighter windows when activity is dense (avoids the cap),
  // wider windows when activity is sparse (fewer round trips).
  const TARGET_ROWS_PER_WINDOW = 100;
  const MIN_WINDOW_MS = 1 * 60 * 60 * 1000; // 1h
  const MAX_WINDOW_MS = 7 * 24 * 60 * 60 * 1000; // 7d
  const INITIAL_WINDOW_MS = 24 * 60 * 60 * 1000; // 24h
  // Current next-window size, evolves as we observe density. Reset by the
  // first fetch's anchor.
  const windowSizeMsRef = useRef<number>(INITIAL_WINDOW_MS);
  // Anchor the floor to the viewed build's time so navigating to old builds
  // gets ~30 days of contemporaneous history rather than nothing.
  const lookbackFloorMs = (currentBuildStartMs ?? Date.now()) - lookbackMs;

  const fetchBuilds = useCallback(async () => {
    if (!username || loading || !queryBuildHistory) return;
    setLoading(true);
    setError(null);

    // First fetch: scan around the current build's time, [build_time - 24h,
    // build_time + 1h]. The +1h cushion accounts for builds whose start_time
    // is recorded slightly before the row lands in Scuba.
    if (explorationRef.current == null) {
      if (currentBuildStartMs != null) {
        // Two-phase initial load: start with a small window [build_time - 3h,
        // build_time + 1h] so the sidebar fills instantly. The auto-prefetch
        // hook below kicks off a second fetch right away that extends
        // pagination back via the normal adaptive flow — no user scroll
        // needed.
        explorationRef.current = {
          windowStartMs: currentBuildStartMs - 3 * 60 * 60 * 1000,
          cursorEndMs: currentBuildStartMs + 60 * 60 * 1000,
          capHits: 0,
        };
      } else {
        const now = Date.now();
        explorationRef.current = {
          windowStartMs: now - windowSizeMsRef.current,
          cursorEndMs: now,
          capHits: 0,
        };
      }
    }

    const {windowStartMs, cursorEndMs, capHits} = explorationRef.current;
    const startTimeMs = windowStartMs;
    const endTimeMs = cursorEndMs;
    try {
      // Filter by hostname on the host side too — without this, CI users get
      // overwhelmed by builds from many machines and the row cap drops most
      // of the user's actual local builds.
      const builds: BuildHistoryRow[] = await queryBuildHistory({
        username,
        hostname,
        startTimeMs,
        endTimeMs,
        limit: FETCH_LIMIT,
      });

      // Track query window bounds regardless of whether builds were returned —
      // we still scanned that range, even if it was empty.
      setSearchedNewestMs(prev =>
        prev == null || endTimeMs > prev ? endTimeMs : prev,
      );
      setSearchedOldestMs(prev =>
        prev == null || startTimeMs < prev ? startTimeMs : prev,
      );

      const hitCap = builds.length >= FETCH_LIMIT;
      if (hitCap && capHits + 1 >= MAX_CAP_HITS_PER_WINDOW) {
        // We've hit the cap too many times draining this single window —
        // give up and move on so the user can keep scrolling. Mark the
        // unscanned [windowStartMs, cursorEndMs] portion as incomplete so
        // it surfaces in the list.
        const oldest = builds.reduce(
          (min, b) => (b.wrapper_start_time < min ? b.wrapper_start_time : min),
          builds[0].wrapper_start_time,
        );
        setIncompleteRanges(prev => [
          ...prev,
          {startMs: windowStartMs, endMs: oldest - 1},
        ]);
        // After bailing out of a dense window, shrink the next window so we
        // hopefully don't immediately hit the cap again.
        windowSizeMsRef.current = Math.max(
          MIN_WINDOW_MS,
          Math.floor(windowSizeMsRef.current / 2),
        );
        const nextWindowStart = windowStartMs - windowSizeMsRef.current;
        if (windowStartMs <= lookbackFloorMs) {
          setReachedFloor(true);
          setHasMore(false);
        } else {
          explorationRef.current = {
            windowStartMs: Math.max(nextWindowStart, lookbackFloorMs),
            cursorEndMs: windowStartMs,
            capHits: 0,
          };
        }
      } else if (hitCap) {
        // Cap-saturated: there are more rows older than what we got but still
        // within [windowStartMs, cursorEndMs]. Don't move to a new window —
        // shrink cursorEndMs to just before the oldest row we got, so the
        // next fetch picks up the next-oldest batch within the same window.
        const oldest = builds.reduce(
          (min, b) => (b.wrapper_start_time < min ? b.wrapper_start_time : min),
          builds[0].wrapper_start_time,
        );
        explorationRef.current = {
          windowStartMs,
          cursorEndMs: oldest - 1,
          capHits: capHits + 1,
        };
      } else {
        // Window fully drained (zero or partial result). Move on to the
        // next-older window. Empty results no longer terminate pagination —
        // the user might have just had a quiet day in the middle of a busy
        // week. Stop only when the next window would cross the look-back
        // floor.
        //
        // Adapt the next window size from the density of THIS fetch (rows
        // per ms over the just-scanned [windowStartMs, cursorEndMs] range).
        // Aim for ~TARGET_ROWS_PER_WINDOW rows per fetch. Smooth toward the
        // ideal by averaging with the current size to avoid wild swings.
        const fetchSpanMs = Math.max(1, cursorEndMs - windowStartMs);
        let nextWindowMs: number;
        if (builds.length === 0) {
          // Empty window — no signal about density. Grow aggressively to
          // skip past sparse periods quickly.
          nextWindowMs = Math.min(windowSizeMsRef.current * 2, MAX_WINDOW_MS);
        } else {
          const idealMs =
            (TARGET_ROWS_PER_WINDOW / builds.length) * fetchSpanMs;
          nextWindowMs = (idealMs + windowSizeMsRef.current) / 2;
        }
        windowSizeMsRef.current = Math.max(
          MIN_WINDOW_MS,
          Math.min(nextWindowMs, MAX_WINDOW_MS),
        );
        const nextWindowStart = windowStartMs - windowSizeMsRef.current;
        if (windowStartMs <= lookbackFloorMs) {
          setReachedFloor(true);
          setHasMore(false);
        } else {
          explorationRef.current = {
            windowStartMs: Math.max(nextWindowStart, lookbackFloorMs),
            cursorEndMs: windowStartMs,
            capHits: 0,
          };
        }
      }

      if (builds.length > 0) {
        setAllBuilds(prev => {
          const seen = new Set(prev.map(b => b.uuid));
          return [...prev, ...builds.filter(b => !seen.has(b.uuid))];
        });
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to load builds');
    } finally {
      setLoading(false);
    }
  }, [
    queryBuildHistory,
    username,
    hostname,
    loading,
    currentBuildStartMs,
    lookbackFloorMs,
  ]);

  // Initial load — wait until we know the current build's time so the first
  // window is centered on it. (currentBuildStartMs is null until the
  // invocation info has loaded.)
  useEffect(() => {
    if (
      username &&
      currentBuildStartMs != null &&
      allBuilds.length === 0 &&
      !loading
    ) {
      fetchBuilds();
    }
  }, [username, currentBuildStartMs]); // eslint-disable-line react-hooks/exhaustive-deps

  // Intersection observer for infinite scroll. The generous rootMargin pre-
  // fires the next fetch when the sentinel is half a viewport away from the
  // bottom, hiding round-trip latency. It also makes the sentinel "in view"
  // immediately after a small initial fetch, which auto-chains the
  // background-expand phase of the two-phase initial load.
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;

    const observer = new IntersectionObserver(
      entries => {
        if (entries[0].isIntersecting && !loading && hasMore) {
          fetchBuilds();
        }
      },
      {threshold: 0, rootMargin: '50% 0px'},
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [loading, hasMore, fetchBuilds]);

  // Filter to matching builds and sort newest-first by start_time
  const filteredBuilds = allBuilds
    .filter(b => {
      if (hostname && b.hostname !== hostname) return false;
      if (isolationDir && b.isolation_dir !== isolationDir) return false;
      if (repository && b.repository !== repository) return false;
      return true;
    })
    .sort((a, b) => b.wrapper_start_time - a.wrapper_start_time);

  // Build the merged list. Walk the sorted builds (newest-first) and slip in
  // boundary markers between any adjacent pair where the daemon_uuid or
  // sandcastle_job_info changes — that's a daemon restart or a sandcastle
  // job switch, both useful for orienting in the timeline. Then merge in
  // incomplete-range markers and sort the whole thing by sortKey desc.
  type ListItem =
    | {kind: 'build'; build: BuildHistoryRow; sortKey: number}
    | {kind: 'incomplete'; startMs: number; endMs: number; sortKey: number}
    | {kind: 'daemon-boundary'; sortKey: number; key: string}
    | {
        kind: 'sandcastle-boundary';
        sortKey: number;
        key: string;
        newerJobId: string | null;
        olderJobId: string | null;
      }
    | {
        kind: 'rebase-boundary';
        sortKey: number;
        key: string;
        newerRev: string | null;
        olderRev: string | null;
      };

  const sandcastleJobId = (info: string | null | undefined): string | null => {
    if (!info) return null;
    const id = info.split('/').pop();
    return id && id.length > 0 ? id : null;
  };

  const buildItems: ListItem[] = [];
  for (let i = 0; i < filteredBuilds.length; i++) {
    const build = filteredBuilds[i];
    buildItems.push({
      kind: 'build',
      build,
      sortKey: build.wrapper_start_time,
    });

    // Boundaries sit between this (newer) build and the next (older) one.
    // Anchor them at the midpoint so they sort cleanly between the two.
    const next = filteredBuilds[i + 1];
    if (!next) continue;
    const boundaryMs = (build.wrapper_start_time + next.wrapper_start_time) / 2;

    if (
      build.daemon_uuid &&
      next.daemon_uuid &&
      build.daemon_uuid !== next.daemon_uuid
    ) {
      buildItems.push({
        kind: 'daemon-boundary',
        sortKey: boundaryMs,
        key: `daemon-${next.uuid}-${build.uuid}`,
      });
    }

    const newerJob = build.sandcastle_job_info ?? '';
    const olderJob = next.sandcastle_job_info ?? '';
    if (newerJob !== olderJob) {
      buildItems.push({
        kind: 'sandcastle-boundary',
        sortKey: boundaryMs,
        key: `sandcastle-${next.uuid}-${build.uuid}`,
        newerJobId: sandcastleJobId(build.sandcastle_job_info),
        olderJobId: sandcastleJobId(next.sandcastle_job_info),
      });
    }

    const newerRev = build.branched_from_revision ?? '';
    const olderRev = next.branched_from_revision ?? '';
    if (newerRev && olderRev && newerRev !== olderRev) {
      buildItems.push({
        kind: 'rebase-boundary',
        sortKey: boundaryMs,
        key: `rebase-${next.uuid}-${build.uuid}`,
        newerRev,
        olderRev,
      });
    }
  }

  const listItems: ListItem[] = [
    ...buildItems,
    ...incompleteRanges.map<ListItem>(r => ({
      kind: 'incomplete',
      startMs: r.startMs,
      endMs: r.endMs,
      sortKey: r.endMs,
    })),
  ].sort((a, b) => b.sortKey - a.sortKey);

  if (!queryBuildHistory) {
    return (
      <div className="text-muted-foreground p-4 text-sm">
        Build history is not available here.
      </div>
    );
  }

  if (!username) {
    return (
      <div className="text-muted-foreground p-3 text-xs">
        Loading build history...
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="border-b p-3">
        <h2 className="text-sm font-semibold">Build History</h2>
        {(hostname || isolationDir || repository) && (
          <div className="text-muted-foreground mt-1 space-y-0.5 text-xs">
            {hostname && (
              <p className="truncate" title={hostname}>
                {hostname}
              </p>
            )}
            {isolationDir && <p>isolation: {isolationDir}</p>}
            {repository && <p>repo: {repository}</p>}
          </div>
        )}
        <div className="text-muted-foreground mt-1 text-xs">
          {filteredBuilds.length} builds
          {allBuilds.length !== filteredBuilds.length && (
            <span> (of {allBuilds.length} total)</span>
          )}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto">
        {error && (
          <div className="p-3 text-xs text-red-600 dark:text-red-400">
            {error}
          </div>
        )}

        {searchedNewestMs != null && (
          <div className="text-muted-foreground border-b border-dashed border-border px-3 py-1.5 text-center text-[11px]">
            Searched as of {formatAbsoluteTime(searchedNewestMs)}
          </div>
        )}

        {listItems.map(item => {
          if (item.kind === 'incomplete') {
            return (
              <div
                key={`incomplete-${item.startMs}-${item.endMs}`}
                className="mx-2 my-1 rounded-md border border-dashed border-yellow-400 bg-yellow-50 px-2 py-1.5 text-[11px] text-yellow-900 dark:border-yellow-600 dark:bg-yellow-950 dark:text-yellow-200"
                title={`We hit the row cap repeatedly while paginating ${new Date(item.startMs).toLocaleString()} – ${new Date(item.endMs).toLocaleString()}, so some builds in this range are not shown.`}>
                <div className="font-medium">⚠ Some builds may be missing</div>
                <div className="text-muted-foreground mt-0.5">
                  {formatAbsoluteTime(item.startMs)} –{' '}
                  {formatAbsoluteTime(item.endMs)}
                </div>
              </div>
            );
          }
          if (item.kind === 'daemon-boundary') {
            return (
              <div
                key={item.key}
                className="mx-2 my-1 flex items-center gap-2 text-[10px] uppercase tracking-wide text-purple-700 dark:text-purple-300"
                title="The Buck2 daemon was restarted between these builds (different daemon_uuid).">
                <span className="h-px flex-1 bg-purple-300 dark:bg-purple-800" />
                <span>↻ daemon restart</span>
                <span className="h-px flex-1 bg-purple-300 dark:bg-purple-800" />
              </div>
            );
          }
          if (item.kind === 'sandcastle-boundary') {
            const {newerJobId, olderJobId} = item;
            const label =
              newerJobId && olderJobId
                ? `sandcastle ${olderJobId} → ${newerJobId}`
                : newerJobId
                  ? `entered sandcastle ${newerJobId}`
                  : olderJobId
                    ? `left sandcastle ${olderJobId}`
                    : 'sandcastle change';
            return (
              <div
                key={item.key}
                className="mx-2 my-1 flex items-center gap-2 text-[10px] uppercase tracking-wide text-teal-700 dark:text-teal-300"
                title="Adjacent builds belong to different sandcastle jobs.">
                <span className="h-px flex-1 bg-teal-300 dark:bg-teal-800" />
                <span>{label}</span>
                <span className="h-px flex-1 bg-teal-300 dark:bg-teal-800" />
              </div>
            );
          }
          if (item.kind === 'rebase-boundary') {
            const shortRev = (rev: string) => rev.slice(0, 10);
            return (
              <div
                key={item.key}
                className="mx-2 my-1 flex items-center gap-2 text-[10px] uppercase tracking-wide text-amber-700 dark:text-amber-300"
                title={`branched_from_revision changed: ${item.olderRev} → ${item.newerRev}`}>
                <span className="h-px flex-1 bg-amber-300 dark:bg-amber-800" />
                <span>
                  ⤴ rebase{' '}
                  <code className="font-mono normal-case">
                    {item.olderRev ? shortRev(item.olderRev) : '?'}
                  </code>{' '}
                  →{' '}
                  <code className="font-mono normal-case">
                    {item.newerRev ? shortRev(item.newerRev) : '?'}
                  </code>
                </span>
                <span className="h-px flex-1 bg-amber-300 dark:bg-amber-800" />
              </div>
            );
          }
          const {build} = item;
          const isCurrent = build.uuid === currentUuid;
          return (
            <button
              key={build.uuid}
              onClick={() => onNavigate(build.uuid)}
              className={`mx-2 my-1 w-[calc(100%-1rem)] rounded-md border-l-2 p-2 text-left shadow-sm transition-colors ${outcomeBorderColor(build.outcome)} ${
                isCurrent
                  ? 'border-2 border-blue-400 bg-blue-50 hover:bg-blue-100 dark:border-blue-600 dark:bg-blue-950 dark:hover:bg-blue-900'
                  : 'bg-white hover:bg-gray-50 dark:bg-gray-900 dark:hover:bg-gray-800'
              }`}>
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground text-[11px]">
                  {formatAbsoluteTime(build.wrapper_start_time)}
                </span>
                {isCurrent && (
                  <Badge variant="secondary" className="text-[10px]">
                    current
                  </Badge>
                )}
              </div>
              <div className="mt-0.5 flex items-center gap-1.5">
                <span className="text-sm font-medium">{build.command}</span>
                {build.target_patterns && (
                  <span className="text-muted-foreground truncate text-xs font-mono">
                    {build.target_patterns}
                  </span>
                )}
              </div>
              <div className="text-muted-foreground mt-0.5 flex items-center gap-2 text-xs">
                <span>{formatDuration(build.duration_ms)}</span>
                {build.max_malloc_bytes_active > 0 && (
                  <span title="Peak malloc-active bytes (jemalloc stats.active) over the lifetime of the build">
                    {formatBytes(build.max_malloc_bytes_active)}
                  </span>
                )}
              </div>
            </button>
          );
        })}

        {searchedOldestMs != null && (
          <div className="text-muted-foreground border-t border-dashed border-border px-3 py-1.5 text-center text-[11px]">
            Searched back to {formatAbsoluteTime(searchedOldestMs)}
            {reachedFloor && ' (30-day limit)'}
          </div>
        )}

        {/* Sentinel for infinite scroll */}
        <div ref={sentinelRef} className="h-8">
          {loading && (
            <p className="text-muted-foreground p-3 text-center text-xs">
              Loading...
            </p>
          )}
          {!hasMore && filteredBuilds.length > 0 && (
            <p className="text-muted-foreground p-3 text-center text-xs">
              {reachedFloor
                ? 'Reached 30-day look-back limit'
                : 'No more builds'}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
