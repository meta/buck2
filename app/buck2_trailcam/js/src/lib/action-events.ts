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

export interface ActionSpan {
  /** Target label like "fbcode//foo/bar:target" */
  targetLabel: string;
  /** Package path like "fbcode//foo/bar" */
  packagePath: string;
  /** Target name like "my_lib" */
  targetName: string;
  /** Configuration like "cfg:dev-linux-x86_64-..." (shortened for display) */
  configuration: string;
  /** Action category (e.g. "cxx_compile", "genrule") */
  category: string;
  /** Action identifier */
  identifier: string;
  /**
   * Full treemap path: package_path / target_name / configuration / category / identifier
   * Package path is split on its components by the treemap builder.
   */
  treemapPath: string;
  /** Execution kind: LOCAL, REMOTE, ACTION_CACHE, etc. */
  executionKind: string;
  /** Whether the action failed */
  failed: boolean;
  durationMs: number;
  wallTimeMs: number;
  outputSizeBytes: number;
  inputFilesSizeBytes: number;
  spanId: number;
}

export type ActionMetric =
  | 'durationMs'
  | 'wallTimeMs'
  | 'outputSizeBytes'
  | 'inputFilesSizeBytes'
  | 'count';

export const ACTION_METRICS: {
  id: ActionMetric;
  label: string;
  format: (v: number) => string;
}[] = [
  {id: 'durationMs', label: 'Duration', format: formatMs},
  {id: 'wallTimeMs', label: 'Wall Time', format: formatMs},
  {id: 'outputSizeBytes', label: 'Output Size', format: formatBytes},
  {id: 'inputFilesSizeBytes', label: 'Input Size', format: formatBytes},
  {id: 'count', label: 'Action Count', format: String},
];

function formatMs(ms: number): string {
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`;
  if (ms < 1000) return `${ms.toFixed(1)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Shorten a buck2 configuration name for display.
 * e.g. "cfg:dev-linux-x86_64-fbcode-abcdef1234" -> "dev-linux-x86_64"
 */
function shortenConfiguration(fullName: string): string {
  // Strip "cfg:" prefix
  let s = fullName.startsWith('cfg:') ? fullName.slice(4) : fullName;
  // Strip hash suffix (last component after last -)
  // Configs look like "dev-linux-x86_64-fbcode-abcdef1234"
  // Keep the meaningful parts, strip the hash-like suffixes
  const parts = s.split('-');
  // Find where hash-like parts start (8+ hex chars)
  const meaningful: string[] = [];
  for (const part of parts) {
    if (/^[0-9a-f]{8,}$/i.test(part)) break;
    meaningful.push(part);
  }
  return meaningful.join('-') || s;
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

export function extractActionSpans(
  summaries: import('./event-summary-store').EventSummaryStore,
  getEventData: (s: EventSummary) => Record<string, unknown>,
): ActionSpan[] {
  const results: ActionSpan[] = [];

  for (let i = 0; i < summaries.length; i++) {
    if (
      summaries.getEventType(i) !== 'actionExecution' ||
      summaries.getType(i) !== 'spanEnd'
    )
      continue;
    const evt = summaries.get(i);
    const data = getEventData(evt);
    const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
    if (!spanEnd?.actionExecution) continue;

    const ae = spanEnd.actionExecution as Record<string, unknown>;
    const duration = spanEnd.duration as Record<string, unknown> | undefined;
    if (!duration) continue;

    // Extract target label, target name, and configuration from key.targetLabel
    let targetLabel = '';
    let packagePath = '';
    let targetName = '';
    let configuration = '';
    const key = ae.key as Record<string, unknown> | undefined;
    const ctl = key?.targetLabel as Record<string, unknown> | undefined;
    if (ctl?.label) {
      const lbl = ctl.label as Record<string, unknown>;
      packagePath = (lbl.package as string) ?? '';
      targetName = (lbl.name as string) ?? '';
      targetLabel = targetName ? `${packagePath}:${targetName}` : packagePath;
    }
    if (ctl?.configuration) {
      const cfg = ctl.configuration as Record<string, unknown>;
      const fullName = (cfg.fullName as string) ?? '';
      // Shorten "cfg:dev-linux-x86_64-fbcode-..." to something readable
      configuration = shortenConfiguration(fullName);
    }

    // Extract action name
    const actionName = ae.name as Record<string, unknown> | undefined;
    const category = (actionName?.category as string) ?? '';
    const identifier = (actionName?.identifier as string) ?? '';

    // Build treemap path: package_path / target_name / configuration / category / identifier
    // The package_path part (e.g. "fbcode//foo/bar") gets split by the treemap builder
    const pathParts = [packagePath || 'unknown'];
    if (targetName) pathParts.push(targetName);
    if (configuration) pathParts.push(configuration);
    if (category) pathParts.push(category);
    if (identifier) pathParts.push(identifier);
    const treemapPath = pathParts.join('/');

    // Extract wall time
    const wallTime = ae.wallTime as Record<string, unknown> | undefined;

    results.push({
      targetLabel,
      packagePath,
      targetName,
      configuration,
      category,
      identifier,
      treemapPath,
      executionKind: (ae.executionKind as string) ?? '',
      failed: !!(ae.failed as boolean),
      durationMs: durationToMs(duration),
      wallTimeMs: wallTime ? durationToMs(wallTime) : 0,
      outputSizeBytes: Number(ae.outputSize ?? 0),
      inputFilesSizeBytes: Number(ae.inputFilesBytes ?? 0),
      spanId: evt.spanId!,
    });
  }

  return results;
}
