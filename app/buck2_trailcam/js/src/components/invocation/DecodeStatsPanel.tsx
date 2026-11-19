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
import {useEventLog} from './EventLogProvider';
import type {PhaseTimings} from '../../lib/phase-timer';
import {totalMs} from '../../lib/phase-timer';

/** Format an events/sec rate, with a 'k' suffix above 10K. */
function fmtRate(eventsPerSec: number): string {
  if (!isFinite(eventsPerSec) || eventsPerSec <= 0) return '—';
  if (eventsPerSec >= 1000)
    return `${(eventsPerSec / 1000).toFixed(1)}k events/s`;
  return `${eventsPerSec.toFixed(0)} events/s`;
}

/** Formats ms as e.g. "1.234s" or "12.3ms". */
function fmt(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(2)}s`;
  if (ms >= 10) return `${ms.toFixed(0)}ms`;
  return `${ms.toFixed(1)}ms`;
}

function fmtBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024)
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

/** Sub-buckets that get grouped into "collectors" (only present in
 *  benchmark CLI runs — the worker omits per-collector timing because the
 *  perf.now overhead exceeds the actual collector work). */
const COLLECTOR_PREFIX = 'collector_';

interface PhaseRow {
  name: string;
  totalMs: number;
  count: number;
}

/**
 * Build the rows we render. We display the top-level buckets, plus a
 * separate per-collector group when those sub-buckets are present.
 */
function buildRows(timings: PhaseTimings): {
  topLevel: PhaseRow[];
  collectors: PhaseRow[];
  topLevelTotal: number;
} {
  const topLevel: PhaseRow[] = [];
  const collectors: PhaseRow[] = [];
  let topLevelTotal = 0;
  for (const [name, t] of Object.entries(timings)) {
    if (name.startsWith(COLLECTOR_PREFIX)) {
      collectors.push({
        name: name.slice(COLLECTOR_PREFIX.length),
        totalMs: t.totalMs,
        count: t.count,
      });
    } else {
      topLevel.push({name, totalMs: t.totalMs, count: t.count});
      topLevelTotal += t.totalMs;
    }
  }
  topLevel.sort((a, b) => b.totalMs - a.totalMs);
  collectors.sort((a, b) => b.totalMs - a.totalMs);
  return {topLevel, collectors, topLevelTotal};
}

const PHASE_COLORS: Record<string, string> = {
  decompress: '#a78bfa',
  varint_parse: '#94a3b8',
  varint_decode_length: '#cbd5e1',
  varint_slice: '#64748b',
  proto_decode: '#3b82f6',
  summary_extract: '#22c55e',
  collectors_total: '#f59e0b',
  idb_writes: '#ec4899',
  cache_read_invocation: '#0ea5e9',
  cache_idb_get_invocation: '#38bdf8',
  cache_idb_update_lastaccessed: '#7dd3fc',
  cache_read_summary_batches: '#6366f1',
  cache_read_chunks: '#a855f7',
  other: '#6b7280',
};

function colorFor(name: string): string {
  return PHASE_COLORS[name] ?? '#94a3b8';
}

function PhaseBar({rows, total}: {rows: PhaseRow[]; total: number}) {
  if (total <= 0) return null;
  return (
    <div className="space-y-1">
      <div className="flex h-2 w-full overflow-hidden rounded bg-gray-100 dark:bg-gray-800">
        {rows.map(r => {
          const pct = (r.totalMs / total) * 100;
          if (pct < 0.5) return null;
          return (
            <div
              key={r.name}
              style={{width: `${pct}%`, backgroundColor: colorFor(r.name)}}
              title={`${r.name}: ${fmt(r.totalMs)} (${pct.toFixed(1)}%)`}
            />
          );
        })}
      </div>
      <div className="grid grid-cols-[1fr_auto_auto] gap-x-3 gap-y-0.5 text-[11px] font-mono">
        {rows.map(r => {
          const pct = total > 0 ? (r.totalMs / total) * 100 : 0;
          return <PhaseRowLine key={r.name} row={r} pct={pct} />;
        })}
      </div>
    </div>
  );
}

function PhaseRowLine({row, pct}: {row: PhaseRow; pct: number}) {
  return (
    <>
      <span className="flex items-center gap-1.5 text-muted-foreground">
        <span
          className="inline-block size-2 shrink-0 rounded-sm"
          style={{backgroundColor: colorFor(row.name)}}
        />
        {row.name}
      </span>
      <span className="text-right">{fmt(row.totalMs)}</span>
      <span className="text-right text-muted-foreground">
        {pct.toFixed(1)}%
      </span>
    </>
  );
}

/**
 * Compact panel showing the live phase-time breakdown of event-log
 * decoding. Renders while loading and stays visible after load
 * completes — the parent gates display behind an explicit debug
 * toggle, so this component does not auto-hide.
 */
export default function DecodeStatsPanel() {
  const logState = useEventLog();
  // Trigger a re-render about every 100ms while loading so the elapsed
  // timer + events/sec display tick along even when no progress message
  // arrives (e.g. mid-decompression with no completed batches yet).
  const [, setTickNonce] = useState(0);
  const isLoading = logState.status === 'loading';
  useEffect(() => {
    if (!isLoading) return;
    const handle = setInterval(() => setTickNonce(n => n + 1), 100);
    return () => clearInterval(handle);
  }, [isLoading]);

  // Nothing useful to show before loading starts.
  if (logState.status === 'idle' || logState.status === 'error') return null;

  const timings = logState.phaseTimings;
  const decompressedBytes =
    logState.status === 'loading'
      ? logState.decompressedBytes
      : logState.status === 'loaded'
        ? logState.decompressedSize
        : undefined;

  const {topLevel, collectors, topLevelTotal} = timings
    ? buildRows(timings)
    : {topLevel: [], collectors: [], topLevelTotal: 0};
  const innerMs = timings ? totalMs(timings) : 0;

  // E2E wall: fetch + decode. While loading, derived live; on loaded,
  // stamped at finish. This is the user-visible "how long did it take".
  const e2eWallMs =
    logState.status === 'loading'
      ? Math.max(0, Date.now() - logState.loadStartedMs)
      : logState.status === 'loaded'
        ? logState.fetchDecodeMs
        : 0;
  const eventsLoaded =
    logState.status === 'loading'
      ? (logState.eventsLoaded ?? 0)
      : logState.status === 'loaded'
        ? logState.totalEvents
        : 0;
  const eventsPerSec = e2eWallMs > 0 ? (eventsLoaded * 1000) / e2eWallMs : 0;

  return (
    <div className="rounded-lg border border-gray-200 bg-gray-50 p-3 text-xs dark:border-gray-800 dark:bg-gray-900/40">
      <div className="mb-2 flex items-center justify-between gap-3">
        <div className="font-medium">
          {isLoading ? 'Decoding event log…' : 'Decode complete'}
        </div>
        <div className="text-muted-foreground font-mono">
          {decompressedBytes != null && (
            <span>{fmtBytes(decompressedBytes)} decompressed · </span>
          )}
          <span>{eventsLoaded.toLocaleString()} events · </span>
          {e2eWallMs > 0 && <span>e2e {fmt(e2eWallMs)} · </span>}
          {eventsPerSec > 0 && <span>{fmtRate(eventsPerSec)}</span>}
        </div>
      </div>

      {topLevelTotal > 0 && <PhaseBar rows={topLevel} total={topLevelTotal} />}

      {innerMs > 0 && innerMs < e2eWallMs * 0.95 && (
        // The phase bar only covers worker-internal phases (the parallel
        // pipeline excludes decoder workers, the single-worker path
        // excludes network fetch). Tag the bar so the gap vs e2e is clear.
        <div className="text-muted-foreground mt-1 text-[10px] font-mono">
          worker-reported phases sum to {fmt(innerMs)}; remaining{' '}
          {fmt(e2eWallMs - innerMs)} is download / decoder work / IDB /
          event-loop yields not in the chart
        </div>
      )}

      {collectors.length > 0 && (
        <div className="mt-3 border-t border-gray-200 pt-2 dark:border-gray-800">
          <div className="text-muted-foreground mb-1 text-[10px] font-semibold uppercase tracking-wide">
            Per-collector
          </div>
          <PhaseBar
            rows={collectors}
            total={collectors.reduce((s, r) => s + r.totalMs, 0)}
          />
        </div>
      )}
    </div>
  );
}
