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

import {useMemo, useState, type ReactNode} from 'react';
import {Badge} from '../../ui';
import {useEventLog} from './EventLogProvider';
import {
  extractTestResults,
  type TestResultInfo,
  type TestResultsSummary,
} from '../../lib/test-events';
import {formatDuration} from '../../lib/format';

type FilterStatus = 'all' | 'failed' | 'passed' | 'skipped' | 'errored';
type SortField = 'name' | 'status' | 'durationMs' | 'targetLabel';
type SortDir = 'asc' | 'desc';

function statusColor(status: string): string {
  switch (status) {
    case 'PASS':
    case 'LISTING_SUCCESS':
      return 'bg-green-100 text-green-700 dark:bg-green-900 dark:text-green-300';
    case 'FAIL':
    case 'LISTING_FAILED':
      return 'bg-red-100 text-red-700 dark:bg-red-900 dark:text-red-300';
    case 'FATAL':
    case 'TIMEOUT':
    case 'INFRA_FAILURE':
      return 'bg-orange-100 text-orange-700 dark:bg-orange-900 dark:text-orange-300';
    case 'SKIP':
    case 'OMITTED':
      return 'bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400';
    default:
      return 'bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400';
  }
}

function statusLabel(status: string): string {
  return status.replace(/_/g, ' ');
}

function matchesFilter(r: TestResultInfo, filter: FilterStatus): boolean {
  if (filter === 'all') return true;
  switch (filter) {
    case 'passed':
      return r.status === 'PASS' || r.status === 'LISTING_SUCCESS';
    case 'failed':
      return r.status === 'FAIL' || r.status === 'LISTING_FAILED';
    case 'errored':
      return (
        r.status === 'FATAL' ||
        r.status === 'TIMEOUT' ||
        r.status === 'INFRA_FAILURE'
      );
    case 'skipped':
      return (
        r.status === 'SKIP' ||
        r.status === 'OMITTED' ||
        r.status === 'UNKNOWN' ||
        r.status === 'RERUN'
      );
  }
}

export default function TestResultsTab({
  testInfraResults,
}: {
  /**
   * Rendered in place of the event-log view when the log points at a run in
   * an external test system; hosts that can reach that system pass their
   * enriched view here.
   */
  testInfraResults?: ReactNode;
}) {
  const logState = useEventLog();

  const testData = useMemo(() => {
    if (logState.status !== 'loaded') return null;
    const result = extractTestResults(
      logState.summaries,
      logState.getEventData,
    );
    return result.total > 0 ? result : null;
  }, [logState]);

  // Check if we have a TestInfra run ID — if so, use the enriched view
  const hasTestInfraRun = useMemo(() => {
    if (!testData?.testConsoleUrl) return false;
    return /\/testrun\/\d+/.test(testData.testConsoleUrl);
  }, [testData]);

  if (logState.status === 'idle' || logState.status === 'loading') {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20">
        <p className="text-sm">
          {logState.status === 'loading' ? logState.progress : 'Loading...'}
        </p>
      </div>
    );
  }

  if (!testData) {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20">
        <p className="text-sm">No test results in this build.</p>
      </div>
    );
  }

  if (hasTestInfraRun && testInfraResults != null) {
    return <>{testInfraResults}</>;
  }

  return <TestResultsView data={testData} />;
}

function TestResultsView({data}: {data: TestResultsSummary}) {
  const [filter, setFilter] = useState<FilterStatus>(
    data.failed > 0 ? 'failed' : 'all',
  );
  const [search, setSearch] = useState('');
  const [sortField, setSortField] = useState<SortField>('name');
  const [sortDir, setSortDir] = useState<SortDir>('asc');
  const [expandedIdxs, setExpandedIdxs] = useState<Set<number>>(
    () => new Set(),
  );

  const toggleExpanded = (idx: number) => {
    setExpandedIdxs(prev => {
      const next = new Set(prev);
      if (next.has(idx)) {
        next.delete(idx);
      } else {
        next.add(idx);
      }
      return next;
    });
  };

  const filteredResults = useMemo(() => {
    let results = data.results.filter(r => matchesFilter(r, filter));
    if (search) {
      const lower = search.toLowerCase();
      results = results.filter(
        r =>
          r.name.toLowerCase().includes(lower) ||
          (r.targetLabel?.toLowerCase().includes(lower) ?? false) ||
          (r.message?.toLowerCase().includes(lower) ?? false),
      );
    }
    results.sort((a, b) => {
      const dir = sortDir === 'asc' ? 1 : -1;
      const av = a[sortField] ?? '';
      const bv = b[sortField] ?? '';
      if (av < bv) return -1 * dir;
      if (av > bv) return 1 * dir;
      return 0;
    });
    return results;
  }, [data.results, filter, search, sortField, sortDir]);

  function toggleSort(field: SortField) {
    if (sortField === field) {
      setSortDir(d => (d === 'asc' ? 'desc' : 'asc'));
    } else {
      setSortField(field);
      setSortDir('asc');
    }
  }

  const filterCounts = {
    all: data.total,
    passed: data.passed,
    failed: data.failed,
    errored: data.errored,
    skipped: data.skipped,
  };

  return (
    <div>
      {/* Summary header */}
      <div className="mb-4 flex items-center justify-between">
        <div className="flex items-center gap-4">
          <span className="text-lg font-semibold">{data.total} tests</span>
          <span className="text-muted-foreground text-sm">
            {formatDuration(data.totalDurationMs)}
          </span>
          {data.testConsoleUrl && (
            <a
              href={data.testConsoleUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="text-xs text-blue-600 hover:underline dark:text-blue-400">
              TestConsole
            </a>
          )}
        </div>

        {/* Pass/fail summary */}
        <div className="flex gap-3 text-sm">
          {data.passed > 0 && (
            <span className="text-green-600 dark:text-green-400">
              {data.passed} passed
            </span>
          )}
          {data.failed > 0 && (
            <span className="font-medium text-red-600 dark:text-red-400">
              {data.failed} failed
            </span>
          )}
          {data.errored > 0 && (
            <span className="text-orange-600 dark:text-orange-400">
              {data.errored} errored
            </span>
          )}
          {data.skipped > 0 && (
            <span className="text-muted-foreground">
              {data.skipped} skipped
            </span>
          )}
        </div>
      </div>

      {/* Pass/fail bar */}
      <div className="mb-4 flex h-2 w-full overflow-hidden rounded-full bg-gray-100 dark:bg-gray-800">
        {data.passed > 0 && (
          <div
            className="bg-green-500"
            style={{width: `${(data.passed / data.total) * 100}%`}}
          />
        )}
        {data.failed > 0 && (
          <div
            className="bg-red-500"
            style={{width: `${(data.failed / data.total) * 100}%`}}
          />
        )}
        {data.errored > 0 && (
          <div
            className="bg-orange-500"
            style={{width: `${(data.errored / data.total) * 100}%`}}
          />
        )}
        {data.skipped > 0 && (
          <div
            className="bg-gray-400"
            style={{width: `${(data.skipped / data.total) * 100}%`}}
          />
        )}
      </div>

      {/* Toolbar */}
      <div className="mb-3 flex items-center gap-3">
        {/* Filter chips */}
        <div className="flex gap-1">
          {(
            ['all', 'failed', 'errored', 'passed', 'skipped'] as FilterStatus[]
          ).map(f =>
            filterCounts[f] > 0 || f === 'all' ? (
              <button
                key={f}
                onClick={() => setFilter(f)}
                className={`rounded px-2 py-0.5 text-xs transition-colors ${
                  filter === f
                    ? 'bg-gray-900 text-white dark:bg-gray-100 dark:text-gray-900'
                    : 'text-muted-foreground hover:bg-gray-100 dark:hover:bg-gray-800'
                }`}>
                {f === 'all' ? 'All' : f.charAt(0).toUpperCase() + f.slice(1)} (
                {filterCounts[f]})
              </button>
            ) : null,
          )}
        </div>

        {/* Search */}
        <input
          type="text"
          placeholder="Search tests..."
          value={search}
          onChange={e => setSearch(e.target.value)}
          className="ml-auto rounded border px-2 py-1 text-xs"
        />
      </div>

      {/* Results table */}
      <div className="rounded border">
        <table className="w-full text-xs">
          <thead className="sticky top-0 z-10 bg-gray-50 dark:bg-gray-900">
            <tr>
              <th
                className="cursor-pointer px-2 py-1.5 text-left font-medium hover:text-foreground"
                onClick={() => toggleSort('status')}>
                Status
                {sortField === 'status' && (sortDir === 'asc' ? ' ↑' : ' ↓')}
              </th>
              <th
                className="cursor-pointer px-2 py-1.5 text-left font-medium hover:text-foreground"
                onClick={() => toggleSort('name')}>
                Test
                {sortField === 'name' && (sortDir === 'asc' ? ' ↑' : ' ↓')}
              </th>
              <th
                className="cursor-pointer px-2 py-1.5 text-left font-medium hover:text-foreground"
                onClick={() => toggleSort('targetLabel')}>
                Target
                {sortField === 'targetLabel' &&
                  (sortDir === 'asc' ? ' ↑' : ' ↓')}
              </th>
              <th
                className="cursor-pointer px-2 py-1.5 text-right font-medium hover:text-foreground"
                onClick={() => toggleSort('durationMs')}>
                Duration
                {sortField === 'durationMs' &&
                  (sortDir === 'asc' ? ' ↑' : ' ↓')}
              </th>
            </tr>
          </thead>
          <tbody>
            {filteredResults.map((r, i) => (
              <TestRow
                key={i}
                result={r}
                expanded={expandedIdxs.has(i)}
                onToggle={() => toggleExpanded(i)}
              />
            ))}
          </tbody>
        </table>
        {filteredResults.length === 0 && (
          <div className="text-muted-foreground py-8 text-center text-sm">
            No tests match the current filters.
          </div>
        )}
      </div>
    </div>
  );
}

function TestRow({
  result,
  expanded,
  onToggle,
}: {
  result: TestResultInfo;
  expanded: boolean;
  onToggle: () => void;
}) {
  return (
    <>
      <tr
        onClick={onToggle}
        className="cursor-pointer border-b border-gray-100 transition-colors hover:bg-gray-50 dark:border-gray-800 dark:hover:bg-gray-800">
        <td className="px-2 py-1.5">
          <Badge
            className={`text-[10px] px-1 py-0 ${statusColor(result.status)}`}>
            {statusLabel(result.status)}
          </Badge>
        </td>
        <td
          className="max-w-md truncate px-2 py-1.5 font-mono"
          title={result.name}>
          {result.name}
        </td>
        <td
          className="text-muted-foreground max-w-xs truncate px-2 py-1.5 font-mono"
          title={result.targetLabel}>
          {result.targetLabel ?? '—'}
        </td>
        <td className="px-2 py-1.5 text-right font-mono">
          {result.durationMs != null ? formatDuration(result.durationMs) : '—'}
        </td>
      </tr>
      {expanded && (
        <tr>
          <td colSpan={4} className="bg-gray-50 px-4 py-3 dark:bg-gray-900">
            <div className="space-y-2 text-xs">
              {result.message && (
                <div>
                  <span className="font-medium">Message: </span>
                  <span className="text-red-600 dark:text-red-400">
                    {result.message}
                  </span>
                </div>
              )}
              {result.details && (
                <div>
                  <span className="font-medium">Output:</span>
                  <pre className="mt-1 whitespace-pre-wrap rounded bg-amber-50 p-2 font-mono text-sm dark:bg-amber-950">
                    {result.details}
                  </pre>
                </div>
              )}
              {result.maxMemoryUsedBytes != null && (
                <div className="text-muted-foreground">
                  Peak memory:{' '}
                  {(result.maxMemoryUsedBytes / (1024 * 1024)).toFixed(1)} MB
                </div>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}
