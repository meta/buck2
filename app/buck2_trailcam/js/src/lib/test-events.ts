/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import type {EventSummary} from './event-log-decoder';

export type TestStatus =
  | 'PASS'
  | 'FAIL'
  | 'SKIP'
  | 'OMITTED'
  | 'FATAL'
  | 'TIMEOUT'
  | 'UNKNOWN'
  | 'RERUN'
  | 'LISTING_SUCCESS'
  | 'LISTING_FAILED'
  | 'INFRA_FAILURE';

export interface TestResultInfo {
  name: string;
  status: TestStatus;
  message?: string;
  details?: string;
  durationMs?: number;
  targetLabel?: string;
  maxMemoryUsedBytes?: number;
}

export interface TestSuiteInfo {
  suiteName: string;
  targetLabel?: string;
  testNames: string[];
}

export interface TestResultsSummary {
  total: number;
  passed: number;
  failed: number;
  skipped: number;
  errored: number;
  /** Total duration of all tests combined */
  totalDurationMs: number;
  /** All individual test results */
  results: TestResultInfo[];
  /** Discovered test suites */
  suites: TestSuiteInfo[];
  /** TestConsole URL if available (from TestSessionInfo) */
  testConsoleUrl?: string;
  /** Test session ID if available */
  testSessionId?: string;
}

function durationToMs(d: Record<string, unknown>): number {
  const seconds =
    typeof d.seconds === 'string'
      ? parseInt(d.seconds, 10)
      : typeof d.seconds === 'number'
        ? d.seconds
        : 0;
  const nanos = typeof d.nanos === 'number' ? d.nanos : 0;
  return seconds * 1000 + nanos / 1e6;
}

function extractTargetLabel(tl: unknown): string | undefined {
  if (!tl || typeof tl !== 'object') return undefined;
  const obj = tl as Record<string, unknown>;
  const label = obj.label as Record<string, unknown> | undefined;
  if (label) {
    const pkg = (label.package as string) ?? '';
    const name = (label.name as string) ?? '';
    return name ? `${pkg}:${name}` : pkg;
  }
  return undefined;
}

/**
 * Extract test results from event log summaries.
 *
 * Filters to testResult, testDiscovery, and endOfTestResults events,
 * decodes only those on demand.
 */
export function extractTestResults(
  summaries: import('./event-summary-store').EventSummaryStore,
  getEventData: (s: EventSummary) => Record<string, unknown>,
): TestResultsSummary {
  const results: TestResultInfo[] = [];
  const suites: TestSuiteInfo[] = [];
  let testConsoleUrl: string | undefined;
  let testSessionId: string | undefined;

  for (let i = 0; i < summaries.length; i++) {
    const et = summaries.getEventType(i);
    if (
      et !== 'testResult' &&
      et !== 'testDiscovery' &&
      et !== 'endOfTestResults'
    )
      continue;
    const evt = summaries.get(i);
    const data = getEventData(evt);
    const instant = data.instant as Record<string, unknown> | undefined;
    if (!instant) continue;

    // TestResult
    const testResult = instant.testResult as
      Record<string, unknown> | undefined;
    if (testResult) {
      const status = (testResult.status as string) ?? 'UNKNOWN';
      const msgObj = testResult.msg as Record<string, unknown> | undefined;
      const duration = testResult.duration as
        Record<string, unknown> | undefined;

      results.push({
        name: (testResult.name as string) ?? '',
        status: status as TestStatus,
        message: msgObj?.msg as string | undefined,
        details: (testResult.details as string) ?? undefined,
        durationMs: duration ? durationToMs(duration) : undefined,
        targetLabel: extractTargetLabel(testResult.targetLabel),
        maxMemoryUsedBytes:
          testResult.maxMemoryUsedBytes != null
            ? Number(testResult.maxMemoryUsedBytes)
            : undefined,
      });
      continue;
    }

    // TestDiscovery
    const testDiscovery = instant.testDiscovery as
      Record<string, unknown> | undefined;
    if (testDiscovery) {
      // Session info (contains test console URL)
      const session = testDiscovery.session as
        Record<string, unknown> | undefined;
      if (session) {
        const info = session.info as string | undefined;
        if (info && !testConsoleUrl) {
          testConsoleUrl = info;
        }
        const sid = session.testSessionId as string | undefined;
        if (sid && !testSessionId) {
          testSessionId = sid;
        }
      }

      // Suite discovery
      const tests = testDiscovery.tests as Record<string, unknown> | undefined;
      if (tests) {
        suites.push({
          suiteName: (tests.suiteName as string) ?? '',
          targetLabel: extractTargetLabel(tests.targetLabel),
          testNames: (tests.testNames as string[]) ?? [],
        });
      }
      continue;
    }
  }

  // Compute summary counts
  let passed = 0;
  let failed = 0;
  let skipped = 0;
  let errored = 0;
  let totalDurationMs = 0;

  for (const r of results) {
    switch (r.status) {
      case 'PASS':
      case 'LISTING_SUCCESS':
        passed++;
        break;
      case 'FAIL':
      case 'LISTING_FAILED':
        failed++;
        break;
      case 'SKIP':
      case 'OMITTED':
        skipped++;
        break;
      case 'FATAL':
      case 'TIMEOUT':
      case 'INFRA_FAILURE':
        errored++;
        break;
      // UNKNOWN, RERUN — count as other/skipped
      default:
        skipped++;
    }
    if (r.durationMs) totalDurationMs += r.durationMs;
  }

  return {
    total: results.length,
    passed,
    failed,
    skipped,
    errored,
    totalDurationMs,
    results,
    suites,
    testConsoleUrl,
    testSessionId,
  };
}
