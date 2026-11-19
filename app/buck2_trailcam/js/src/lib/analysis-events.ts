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
import {formatBytes} from './format';

export interface AnalysisSpan {
  /** Target label like "fbcode//foo/bar:target" */
  targetLabel: string;
  /** Package path like "fbcode//foo/bar" */
  packagePath: string;
  /** Rule type like "cxx_library" */
  rule: string;
  durationMs: number;
  declaredActions: number;
  declaredArtifacts: number;
  /** Starlark heap bytes retained by this target's analysis result
   * (AnalysisProfile.starlark_allocated_bytes). 0 if not reported. */
  retainedMemoryBytes: number;
  spanId: number;
}

export type AnalysisMetric =
  | 'durationMs'
  | 'declaredActions'
  | 'declaredArtifacts'
  | 'retainedMemoryBytes'
  | 'count';

export const ANALYSIS_METRICS: {
  id: AnalysisMetric;
  label: string;
  format: (v: number) => string;
}[] = [
  {id: 'durationMs', label: 'Duration', format: formatMs},
  {id: 'declaredActions', label: 'Declared Actions', format: String},
  {id: 'declaredArtifacts', label: 'Declared Artifacts', format: String},
  {id: 'retainedMemoryBytes', label: 'Retained Memory', format: formatBytes},
  {id: 'count', label: 'Target Count', format: String},
];

function formatMs(ms: number): string {
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`;
  if (ms < 1000) return `${ms.toFixed(1)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
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

function extractTargetLabel(
  target: Record<string, unknown>,
): {label: string; packagePath: string} | null {
  // ConfiguredTargetLabel has label: { package, name }
  const lbl = target.label as Record<string, unknown> | undefined;
  if (lbl) {
    const pkg = (lbl.package as string | undefined) ?? '';
    const name = (lbl.name as string | undefined) ?? '';
    return {
      label: name ? `${pkg}:${name}` : pkg,
      packagePath: pkg,
    };
  }
  // Might be directly { package, name }
  if (typeof target.package === 'string') {
    const pkg = target.package as string;
    const name = (target.name as string | undefined) ?? '';
    return {
      label: name ? `${pkg}:${name}` : pkg,
      packagePath: pkg,
    };
  }
  return null;
}

export function extractAnalysisSpans(
  summaries: import('./event-summary-store').EventSummaryStore,
  getEventData: (s: EventSummary) => Record<string, unknown>,
): AnalysisSpan[] {
  const results: AnalysisSpan[] = [];

  for (let i = 0; i < summaries.length; i++) {
    if (
      summaries.getEventType(i) !== 'analysis' ||
      summaries.getType(i) !== 'spanEnd'
    )
      continue;
    const spanId = summaries.getSpanId(i);
    if (spanId == null) continue;
    const evt = summaries.get(i);
    const data = getEventData(evt);
    const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
    if (!spanEnd?.analysis) continue;

    const analysis = spanEnd.analysis as Record<string, unknown>;
    const duration = spanEnd.duration as Record<string, unknown> | undefined;
    if (!duration) continue;

    // Extract target label from the oneof
    const standardTarget = analysis.standardTarget as
      Record<string, unknown> | undefined;
    const targetInfo = standardTarget
      ? extractTargetLabel(standardTarget)
      : null;
    if (!targetInfo) continue; // skip anon targets and dynamic lambdas for now

    const profile = analysis.profile as Record<string, unknown> | undefined;
    results.push({
      targetLabel: targetInfo.label,
      packagePath: targetInfo.packagePath,
      rule: (analysis.rule as string) ?? '',
      durationMs: durationToMs(duration),
      declaredActions: Number(analysis.declaredActions ?? 0),
      declaredArtifacts: Number(analysis.declaredArtifacts ?? 0),
      retainedMemoryBytes: Number(profile?.starlarkAllocatedBytes ?? 0),
      spanId,
    });
  }

  return results;
}
