/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

/**
 * Streaming collectors process events inline during the streaming decode pass.
 * Each collector accumulates data for a specific feature (treemaps, critical path, etc.)
 * so the main thread doesn't need to re-decode events for these features.
 */

import type {EventSummary} from './event-log-decoder';
import type {LoadPackageSpan} from './load-events';
import type {ActionSpan} from './action-events';
import type {AnalysisSpan} from './analysis-events';
import {
  type CriticalPathData,
  parseCriticalPathFromBgInfo,
} from './critical-path';

// Re-export the aggregate data type
export interface AggregateData {
  loadSpans: LoadPackageSpan[];
  actionSpans: ActionSpan[];
  analysisSpans: AnalysisSpan[];
  criticalPath: CriticalPathData | null;
}

export interface StreamingCollector {
  /** Process a single decoded event. Called for every event during streaming. */
  processEvent(summary: EventSummary, data: Record<string, unknown>): void;
}

// ============================================================================
// Helpers (duplicated from individual modules to avoid circular deps)
// ============================================================================

function durationToMs(d: unknown): number {
  if (!d || typeof d !== 'object') return 0;
  const obj = d as Record<string, unknown>;
  const seconds =
    typeof obj.seconds === 'string'
      ? parseInt(obj.seconds, 10)
      : typeof obj.seconds === 'number'
        ? obj.seconds
        : 0;
  const nanos = typeof obj.nanos === 'number' ? obj.nanos : 0;
  return seconds * 1000 + nanos / 1e6;
}

function asOptionalNumber(v: unknown): number | undefined {
  if (v == null) return undefined;
  const n = Number(v);
  return isNaN(n) ? undefined : n;
}

function extractTargetLabelFromConfigured(
  target: unknown,
): {label: string; packagePath: string} | null {
  if (!target || typeof target !== 'object') return null;
  const t = target as Record<string, unknown>;
  const lbl = t.label as Record<string, unknown> | undefined;
  if (!lbl) return null;
  const pkg = (lbl.package as string) ?? '';
  const name = (lbl.name as string) ?? '';
  return {label: name ? `${pkg}:${name}` : pkg, packagePath: pkg};
}

function shortenConfiguration(fullName: string): string {
  let s = fullName.startsWith('cfg:') ? fullName.slice(4) : fullName;
  const parts = s.split('-');
  const meaningful: string[] = [];
  for (const part of parts) {
    if (/^[0-9a-f]{8,}$/i.test(part)) break;
    meaningful.push(part);
  }
  return meaningful.join('-') || s;
}

// ============================================================================
// Load Package Collector
// ============================================================================

export class LoadPackageCollector implements StreamingCollector {
  private startPaths = new Map<number, string>();
  private buildFileMetrics = new Map<
    string,
    {
      starlarkPeakAllocatedBytes?: number;
      cpuInstructionCount?: number;
      targetCount?: number;
    }
  >();
  readonly results: LoadPackageSpan[] = [];

  processEvent(summary: EventSummary, data: Record<string, unknown>): void {
    if (summary.spanId == null) return;

    const spanStart = data.spanStart as Record<string, unknown> | undefined;
    if (spanStart?.loadPackage) {
      const lp = spanStart.loadPackage as Record<string, unknown>;
      if (typeof lp.path === 'string') {
        this.startPaths.set(summary.spanId, lp.path);
      }
      return;
    }

    const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
    if (!spanEnd) return;

    if (spanEnd.load) {
      const lbf = spanEnd.load as Record<string, unknown>;
      const rawModuleId = (lbf.moduleId ?? lbf.module_id) as string | undefined;
      if (rawModuleId) {
        const moduleId = rawModuleId.replace(/:[^/]+$/, '');
        this.buildFileMetrics.set(moduleId, {
          starlarkPeakAllocatedBytes: asOptionalNumber(
            lbf.starlarkPeakAllocatedBytes ?? lbf.starlark_peak_allocated_bytes,
          ),
          cpuInstructionCount: asOptionalNumber(
            lbf.cpuInstructionCount ?? lbf.cpu_instruction_count,
          ),
          targetCount: asOptionalNumber(lbf.targetCount ?? lbf.target_count),
        });
      }
    }

    if (spanEnd.loadPackage) {
      const lp = spanEnd.loadPackage as Record<string, unknown>;
      const path =
        this.startPaths.get(summary.spanId) ??
        (typeof lp.path === 'string' ? lp.path : null);
      const duration = spanEnd.duration as Record<string, unknown> | undefined;
      if (path && duration) {
        this.results.push({
          path,
          durationMs: durationToMs(duration),
          spanId: summary.spanId,
        });
      }
    }
  }

  finalize(): void {
    // Merge LoadBuildFile metrics onto LoadPackage spans
    for (const span of this.results) {
      const metrics = this.buildFileMetrics.get(span.path);
      if (metrics) {
        span.starlarkPeakAllocatedBytes = metrics.starlarkPeakAllocatedBytes;
        span.cpuInstructionCount = metrics.cpuInstructionCount;
        span.targetCount = metrics.targetCount;
      }
    }
  }
}

// ============================================================================
// Analysis Span Collector
// ============================================================================

export class AnalysisSpanCollector implements StreamingCollector {
  readonly results: AnalysisSpan[] = [];

  processEvent(summary: EventSummary, data: Record<string, unknown>): void {
    if (
      summary.eventType !== 'analysis' ||
      summary.type !== 'spanEnd' ||
      summary.spanId == null
    )
      return;

    const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
    if (!spanEnd?.analysis) return;

    const analysis = spanEnd.analysis as Record<string, unknown>;
    const duration = spanEnd.duration as Record<string, unknown> | undefined;
    if (!duration) return;

    const standardTarget = analysis.standardTarget as
      Record<string, unknown> | undefined;
    const targetInfo = standardTarget
      ? extractTargetLabelFromConfigured(standardTarget)
      : null;
    if (!targetInfo) return;

    const profile = analysis.profile as Record<string, unknown> | undefined;
    this.results.push({
      targetLabel: targetInfo.label,
      packagePath: targetInfo.packagePath,
      rule: (analysis.rule as string) ?? '',
      durationMs: durationToMs(duration),
      declaredActions: Number(analysis.declaredActions ?? 0),
      declaredArtifacts: Number(analysis.declaredArtifacts ?? 0),
      retainedMemoryBytes: Number(profile?.starlarkAllocatedBytes ?? 0),
      spanId: summary.spanId,
    });
  }
}

// ============================================================================
// Action Span Collector
// ============================================================================

export class ActionSpanCollector implements StreamingCollector {
  readonly results: ActionSpan[] = [];

  processEvent(summary: EventSummary, data: Record<string, unknown>): void {
    if (
      summary.eventType !== 'actionExecution' ||
      summary.type !== 'spanEnd' ||
      summary.spanId == null
    )
      return;

    const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
    if (!spanEnd?.actionExecution) return;

    const ae = spanEnd.actionExecution as Record<string, unknown>;
    const duration = spanEnd.duration as Record<string, unknown> | undefined;
    if (!duration) return;

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
      configuration = shortenConfiguration((cfg.fullName as string) ?? '');
    }

    const actionName = ae.name as Record<string, unknown> | undefined;
    const category = (actionName?.category as string) ?? '';
    const identifier = (actionName?.identifier as string) ?? '';

    const pathParts = [packagePath || 'unknown'];
    if (targetName) pathParts.push(targetName);
    if (configuration) pathParts.push(configuration);
    if (category) pathParts.push(category);
    if (identifier) pathParts.push(identifier);

    const wallTime = ae.wallTime as Record<string, unknown> | undefined;

    this.results.push({
      targetLabel,
      packagePath,
      targetName,
      configuration,
      category,
      identifier,
      treemapPath: pathParts.join('/'),
      executionKind: (ae.executionKind as string) ?? '',
      failed: !!(ae.failed as boolean),
      durationMs: durationToMs(duration),
      wallTimeMs: wallTime ? durationToMs(wallTime) : 0,
      outputSizeBytes: Number(ae.outputSize ?? 0),
      inputFilesSizeBytes: Number(ae.inputFilesBytes ?? 0),
      spanId: summary.spanId,
    });
  }
}

// ============================================================================
// Critical Path Collector
// ============================================================================

export class CriticalPathCollector implements StreamingCollector {
  result: CriticalPathData | null = null;

  processEvent(summary: EventSummary, data: Record<string, unknown>): void {
    if (this.result) return; // already found it
    if (summary.type !== 'instant' || summary.eventType !== 'buildGraphInfo')
      return;

    const instant = data.instant as Record<string, unknown> | undefined;
    if (!instant) return;
    const bgInfo = (instant.buildGraphInfo ?? instant.build_graph_info) as
      Record<string, unknown> | undefined;
    if (!bgInfo) return;

    // Parse the raw bgInfo into structured CriticalPathData (PhaseGroup[] etc.)
    // so the main thread can consume it directly without re-parsing.
    this.result = parseCriticalPathFromBgInfo(bgInfo);
  }
}
