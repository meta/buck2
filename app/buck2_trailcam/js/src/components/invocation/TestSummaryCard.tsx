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

import {useMemo, useState} from 'react';
import {Card, CardContent, CardHeader, CardTitle} from '../../ui';
import {useEventLog} from './EventLogProvider';
import {extractTestResults} from '../../lib/test-events';
import {formatDuration} from '../../lib/format';
import {useUrlState} from '../../lib/url-state';

function StatusDot({color}: {color: string}) {
  return <span className={`inline-block size-2.5 rounded-full ${color}`} />;
}

const INITIAL_FAILURE_LIMIT = 3;

export default function TestSummaryCard() {
  const logState = useEventLog();
  const [expanded, setExpanded] = useState(false);
  const [, setTab] = useUrlState('tab', 'overview');

  const testData = useMemo(() => {
    if (logState.status !== 'loaded') return null;
    const result = extractTestResults(
      logState.summaries,
      logState.getEventData,
    );
    return result.total > 0 ? result : null;
  }, [logState]);

  const failureResults = useMemo(() => {
    if (!testData) return [];
    return testData.results.filter(
      r =>
        r.status === 'FAIL' ||
        r.status === 'LISTING_FAILED' ||
        r.status === 'FATAL' ||
        r.status === 'TIMEOUT' ||
        r.status === 'INFRA_FAILURE',
    );
  }, [testData]);

  if (!testData) return null;

  const allPassed = testData.failed === 0 && testData.errored === 0;
  const hasFailures = testData.failed > 0 || testData.errored > 0;
  const visibleFailures = expanded
    ? failureResults
    : failureResults.slice(0, INITIAL_FAILURE_LIMIT);
  const hiddenFailureCount = failureResults.length - visibleFailures.length;

  return (
    <Card className={hasFailures ? 'border-red-200 dark:border-red-800' : ''}>
      <CardHeader className="pb-2">
        <CardTitle className="flex items-center gap-2 text-base">
          Test Results
          {allPassed && <StatusDot color="bg-green-500" />}
          {hasFailures && <StatusDot color="bg-red-500" />}
          <button
            onClick={() => setTab('tests')}
            className="group ml-3 inline-flex h-7 items-center rounded-full bg-[var(--secondary)] p-[3px] text-[var(--foreground)]">
            <span className="inline-flex h-full items-center rounded-full px-3 text-sm font-normal transition-colors group-hover:bg-[var(--background)]/70">
              View details →
            </span>
          </button>
        </CardTitle>
      </CardHeader>
      <CardContent>
        {/* Summary bar */}
        <div className="flex items-center gap-4 text-sm">
          <span className="font-medium">{testData.total} tests</span>
          <span className="text-muted-foreground">
            {formatDuration(testData.totalDurationMs)}
          </span>
        </div>

        {/* Counts */}
        <div className="mt-2 flex gap-4 text-xs">
          {testData.passed > 0 && (
            <span className="flex items-center gap-1">
              <StatusDot color="bg-green-500" />
              {testData.passed} passed
            </span>
          )}
          {testData.failed > 0 && (
            <span className="flex items-center gap-1">
              <StatusDot color="bg-red-500" />
              {testData.failed} failed
            </span>
          )}
          {testData.errored > 0 && (
            <span className="flex items-center gap-1">
              <StatusDot color="bg-orange-500" />
              {testData.errored} errored
            </span>
          )}
          {testData.skipped > 0 && (
            <span className="flex items-center gap-1">
              <StatusDot color="bg-gray-400" />
              {testData.skipped} skipped
            </span>
          )}
        </div>

        {/* Pass/fail bar */}
        {testData.total > 0 && (
          <div className="mt-2 flex h-2 w-full overflow-hidden rounded-full bg-gray-100 dark:bg-gray-800">
            {testData.passed > 0 && (
              <div
                className="bg-green-500"
                style={{width: `${(testData.passed / testData.total) * 100}%`}}
              />
            )}
            {testData.failed > 0 && (
              <div
                className="bg-red-500"
                style={{width: `${(testData.failed / testData.total) * 100}%`}}
              />
            )}
            {testData.errored > 0 && (
              <div
                className="bg-orange-500"
                style={{width: `${(testData.errored / testData.total) * 100}%`}}
              />
            )}
            {testData.skipped > 0 && (
              <div
                className="bg-gray-400"
                style={{width: `${(testData.skipped / testData.total) * 100}%`}}
              />
            )}
          </div>
        )}

        {/* Failures inline (includes FAIL, LISTING_FAILED, FATAL, TIMEOUT, INFRA_FAILURE) */}
        {failureResults.length > 0 && (
          <div className="mt-3 space-y-1">
            {visibleFailures.map((r, i) => {
              const isErrored =
                r.status === 'FATAL' ||
                r.status === 'TIMEOUT' ||
                r.status === 'INFRA_FAILURE';
              const containerClass = isErrored
                ? 'rounded bg-orange-50 px-2 py-1 font-mono text-xs text-orange-700 dark:bg-orange-950 dark:text-orange-300'
                : 'rounded bg-red-50 px-2 py-1 font-mono text-xs text-red-700 dark:bg-red-950 dark:text-red-300';
              const accentClass = isErrored
                ? 'text-orange-500'
                : 'text-red-500';
              return (
                <div
                  key={i}
                  className={`truncate ${containerClass}`}
                  title={r.message ?? r.name}>
                  <span className={`mr-1 uppercase ${accentClass}`}>
                    [{r.status.replace(/_/g, ' ')}]
                  </span>
                  {r.name}
                  {r.message && (
                    <span className={accentClass}> — {r.message}</span>
                  )}
                </div>
              );
            })}
            {failureResults.length > INITIAL_FAILURE_LIMIT && (
              <button
                onClick={() => setExpanded(e => !e)}
                className="text-muted-foreground hover:text-foreground text-xs underline">
                {expanded
                  ? 'Show less'
                  : `Show ${hiddenFailureCount} more failure${hiddenFailureCount === 1 ? '' : 's'}`}
              </button>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
