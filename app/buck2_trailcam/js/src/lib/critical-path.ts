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

// --- Types ---

export type CriticalPathPhase =
  | 'startup'
  | 'load'
  | 'analysis'
  | 'execution'
  | 'materialization'
  | 'testing'
  | 'shutdown';

export type CriticalPathEntryType =
  | 'analysis'
  | 'action_execution'
  | 'load'
  | 'listing'
  | 'waiting'
  | 'generic_entry'
  | 'final_materialization'
  | 'test_execution'
  | 'test_listing'
  | 'compute_critical_path'
  | 'dynamic_analysis'
  | 'ensure_transitive_set_projection'
  | 'unknown';

export interface CriticalPathEntry {
  type: CriticalPathEntryType;
  phase: CriticalPathPhase;
  label: string;
  sublabel?: string;
  startOffsetMs: number;
  /** Total wall time: critical path time + non-critical path time */
  wallDurationMs: number;
  /** Time on the critical path only (proto `duration` field) */
  criticalDurationMs: number;
  userDurationMs: number;
  totalDurationMs: number;
  queueDurationMs?: number;
  nonCriticalPathDurationMs?: number;
  potentialImprovementMs?: number;
  executionKind?: string;
  /** Category from action_execution.name (e.g. cxx_compile, write) */
  actionCategory?: string;
  waitingCategory?: string;
  /** True if phase should be resolved from neighboring entries */
  _needsContextPhase?: boolean;
  /** Raw proto entry (kept around so the details view can show a pretty
   *  JSON dump on hover). The parsed fields above are derived from this. */
  _raw?: Record<string, unknown>;
}

export interface PhaseGroup {
  phase: CriticalPathPhase;
  entries: CriticalPathEntry[];
  totalDurationMs: number;
}

export interface TopLevelTarget {
  label: string;
  durationMs: number;
}

export interface CriticalPathData {
  criticalPath: PhaseGroup[];
  slowestPath: PhaseGroup[];
  topLevelTargets: TopLevelTarget[];
  totalDurationMs: number;
  slowestTotalMs: number;
}

// --- Phase colors ---

export const PHASE_COLORS: Record<CriticalPathPhase, string> = {
  startup: '#94a3b8',
  load: '#a78bfa',
  analysis: '#3b82f6',
  execution: '#22c55e',
  materialization: '#f59e0b',
  testing: '#ec4899',
  shutdown: '#94a3b8',
};

export const PHASE_LABELS: Record<CriticalPathPhase, string> = {
  startup: 'Startup',
  load: 'Load',
  analysis: 'Analysis',
  execution: 'Execution',
  materialization: 'Materialization',
  testing: 'Testing',
  shutdown: 'Shutdown',
};

// --- Helpers ---

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

/** Generic entry kinds that map to specific phases instead of startup */
const GENERIC_ENTRY_PHASE_OVERRIDES: Record<string, CriticalPathPhase | null> =
  {
    configure_target: 'load',
    build_key: 'execution',
    buckd_command_init: 'startup',
    'file-watcher-wait': 'startup',
    'other-command-start-overhead': 'startup',
  };

function classifyPhase(
  type: CriticalPathEntryType,
  genericKind?: string,
): CriticalPathPhase | null {
  switch (type) {
    case 'generic_entry':
      if (genericKind && genericKind in GENERIC_ENTRY_PHASE_OVERRIDES) {
        return GENERIC_ENTRY_PHASE_OVERRIDES[genericKind];
      }
      // Default: context-dependent like waiting
      return null;
    case 'load':
    case 'listing':
      return 'load';
    case 'analysis':
      return 'analysis';
    case 'action_execution':
    case 'ensure_transitive_set_projection':
      return 'execution';
    case 'compute_critical_path':
      return 'shutdown';
    case 'final_materialization':
      return 'materialization';
    case 'test_execution':
    case 'test_listing':
      return 'testing';
    // These get their phase from context:
    case 'dynamic_analysis':
      return null;
    case 'waiting':
      return null;
    case 'unknown':
      return null;
    default:
      return 'startup';
  }
}

function extractTargetLabel(target: unknown): string {
  if (!target || typeof target !== 'object') return '';
  const t = target as Record<string, unknown>;
  const label = t.label as Record<string, unknown> | undefined;
  if (!label) return '';
  const pkg = (label.package as string) ?? '';
  const name = (label.name as string) ?? '';
  return name ? `${pkg}:${name}` : pkg;
}

function parseEntry(raw: Record<string, unknown>): CriticalPathEntry {
  const startOffsetNs = Number(raw.startOffsetNs ?? raw.start_offset_ns ?? 0);

  // Determine entry type and extract label
  let type: CriticalPathEntryType = 'unknown';
  let label = '';
  let sublabel: string | undefined;
  let executionKind: string | undefined;
  let actionCategory: string | undefined;
  let waitingCategory: string | undefined;
  let genericEntryKind: string | undefined;

  if (raw.actionExecution || raw.action_execution) {
    type = 'action_execution';
    const ae = (raw.actionExecution ?? raw.action_execution) as Record<
      string,
      unknown
    >;
    const targetLabel = ae.targetLabel ?? ae.target_label;
    label = extractTargetLabel(targetLabel);
    const name = ae.name as Record<string, unknown> | undefined;
    if (name) {
      const cat = (name.category as string) ?? '';
      const id = (name.identifier as string) ?? '';
      if (cat) actionCategory = cat;
      sublabel = [cat, id].filter(Boolean).join(' ');
    }
    executionKind = (ae.executionKind ?? ae.execution_kind) as
      string | undefined;
    if (ae.targetRuleTypeName ?? ae.target_rule_type_name) {
      sublabel =
        `${sublabel ?? ''} (${ae.targetRuleTypeName ?? ae.target_rule_type_name})`.trim();
    }
  } else if (raw.analysis) {
    type = 'analysis';
    const a = raw.analysis as Record<string, unknown>;
    const target = a.standardTarget ?? a.standard_target;
    label = extractTargetLabel(target);
    if (a.targetRuleTypeName ?? a.target_rule_type_name) {
      sublabel = (a.targetRuleTypeName ?? a.target_rule_type_name) as string;
    }
  } else if (raw.dynamicAnalysis ?? raw.dynamic_analysis) {
    type = 'dynamic_analysis';
    const da = (raw.dynamicAnalysis ?? raw.dynamic_analysis) as Record<
      string,
      unknown
    >;
    const target = da.standardTarget ?? da.standard_target;
    label = extractTargetLabel(target);
  } else if (raw.load) {
    type = 'load';
    const l = raw.load as Record<string, unknown>;
    label = (l.package as string) ?? '';
  } else if (raw.listing) {
    type = 'listing';
    const l = raw.listing as Record<string, unknown>;
    label = (l.package as string) ?? '';
  } else if (raw.waiting) {
    type = 'waiting';
    const w = raw.waiting as Record<string, unknown>;
    waitingCategory = (w.category as string) ?? undefined;
    label = waitingCategory ?? 'waiting';
  } else if (raw.genericEntry ?? raw.generic_entry) {
    type = 'generic_entry';
    const g = (raw.genericEntry ?? raw.generic_entry) as Record<
      string,
      unknown
    >;
    const kind = (g.kind as string) ?? 'overhead';
    label = kind;
    genericEntryKind = kind;
  } else if (raw.finalMaterialization ?? raw.final_materialization) {
    type = 'final_materialization';
    const fm = (raw.finalMaterialization ??
      raw.final_materialization) as Record<string, unknown>;
    const target = fm.targetLabel ?? fm.target_label;
    label = extractTargetLabel(target) || ((fm.path as string) ?? '');
  } else if (raw.testExecution ?? raw.test_execution) {
    type = 'test_execution';
    const te = (raw.testExecution ?? raw.test_execution) as Record<
      string,
      unknown
    >;
    label = extractTargetLabel(te.targetLabel ?? te.target_label);
    sublabel = (te.suite as string) ?? undefined;
  } else if (raw.testListing ?? raw.test_listing) {
    type = 'test_listing';
    const tl = (raw.testListing ?? raw.test_listing) as Record<string, unknown>;
    label = extractTargetLabel(tl.targetLabel ?? tl.target_label);
  } else if (raw.computeCriticalPath ?? raw.compute_critical_path) {
    type = 'compute_critical_path';
    label = 'compute critical path';
  } else if (
    raw.ensureTransitiveSetProjection ??
    raw.ensure_transitive_set_projection
  ) {
    type = 'ensure_transitive_set_projection';
    label = 'ensure transitive set projection';
  }

  const classifiedPhase = classifyPhase(type, genericEntryKind);
  const phase = classifiedPhase ?? 'execution'; // placeholder, resolved in groupByPhase

  const criticalDurationMs = durationToMs(raw.duration);
  const nonCriticalPathDurationMs =
    raw.nonCriticalPathDuration || raw.non_critical_path_duration
      ? durationToMs(
          raw.nonCriticalPathDuration ?? raw.non_critical_path_duration,
        )
      : undefined;

  return {
    type,
    phase,
    label,
    sublabel,
    startOffsetMs: startOffsetNs / 1e6,
    wallDurationMs: criticalDurationMs + (nonCriticalPathDurationMs ?? 0),
    criticalDurationMs,
    userDurationMs: durationToMs(raw.userDuration ?? raw.user_duration),
    totalDurationMs: durationToMs(raw.totalDuration ?? raw.total_duration),
    queueDurationMs:
      raw.queueDuration || raw.queue_duration
        ? durationToMs(raw.queueDuration ?? raw.queue_duration)
        : undefined,
    nonCriticalPathDurationMs,
    potentialImprovementMs:
      raw.potentialImprovementDuration || raw.potential_improvement_duration
        ? durationToMs(
            raw.potentialImprovementDuration ??
              raw.potential_improvement_duration,
          )
        : undefined,
    executionKind,
    actionCategory,
    waitingCategory,
    _needsContextPhase: classifiedPhase === null,
    _raw: raw,
  };
}

/** Check if an entry needs its phase resolved from context */
function needsContextPhase(entry: CriticalPathEntry): boolean {
  return entry._needsContextPhase === true;
}

/** For dynamic_analysis, restrict to analysis or execution */
function resolveContextPhase(
  type: CriticalPathEntryType,
  neighborPhase: CriticalPathPhase,
): CriticalPathPhase {
  if (type === 'dynamic_analysis') {
    // dynamic_analysis should only be analysis or execution
    return neighborPhase === 'analysis' || neighborPhase === 'execution'
      ? neighborPhase
      : 'analysis';
  }
  return neighborPhase;
}

function groupByPhase(entries: CriticalPathEntry[]): PhaseGroup[] {
  if (entries.length === 0) return [];

  // Pass 1: Assign context-dependent phases.
  // waiting → phase of the NEXT non-context entry (looks forward)
  // dynamic_analysis → analysis or execution from nearest non-context neighbor
  const resolved = entries.map(e => ({...e}));
  for (let i = resolved.length - 1; i >= 0; i--) {
    if (!needsContextPhase(resolved[i])) continue;
    // Look forward for the next entry with a fixed phase
    let neighborPhase: CriticalPathPhase | null = null;
    for (let j = i + 1; j < resolved.length; j++) {
      if (!needsContextPhase(resolved[j])) {
        neighborPhase = resolved[j].phase;
        break;
      }
    }
    // Fall back to looking backward
    if (!neighborPhase) {
      for (let j = i - 1; j >= 0; j--) {
        if (!needsContextPhase(resolved[j])) {
          neighborPhase = resolved[j].phase;
          break;
        }
      }
    }
    if (neighborPhase) {
      resolved[i].phase = resolveContextPhase(resolved[i].type, neighborPhase);
    }
  }

  // Pass 2: Group consecutive entries by phase
  const groups: PhaseGroup[] = [];
  let currentPhase = resolved[0].phase;
  let currentEntries: CriticalPathEntry[] = [];

  for (const entry of resolved) {
    if (entry.phase !== currentPhase) {
      if (currentEntries.length > 0) {
        groups.push({
          phase: currentPhase,
          entries: currentEntries,
          totalDurationMs: currentEntries.reduce(
            (s, e) => s + e.wallDurationMs,
            0,
          ),
        });
      }
      currentPhase = entry.phase;
      currentEntries = [];
    }
    currentEntries.push(entry);
  }

  if (currentEntries.length > 0) {
    groups.push({
      phase: currentPhase,
      entries: currentEntries,
      totalDurationMs: currentEntries.reduce((s, e) => s + e.wallDurationMs, 0),
    });
  }

  return groups;
}

function parseTopLevelTargets(raw: unknown[]): TopLevelTarget[] {
  return raw
    .map(t => {
      const obj = t as Record<string, unknown>;
      return {
        label: extractTargetLabel(obj.target),
        durationMs: durationToMs(obj.duration),
      };
    })
    .sort((a, b) => b.durationMs - a.durationMs);
}

// --- Main extraction ---

/**
 * Parse a raw protobuf BuildGraphInfo object into the structured
 * CriticalPathData (with PhaseGroup[] etc.) consumed by the UI.
 * Used by both the on-demand main-thread extractor and the worker
 * CriticalPathCollector finalize step.
 */
export function parseCriticalPathFromBgInfo(
  bgInfo: Record<string, unknown>,
): CriticalPathData {
  const rawCriticalPath =
    ((bgInfo.criticalPath2 ?? bgInfo.critical_path2) as
      Record<string, unknown>[] | undefined) ?? [];
  const rawSlowestPath =
    ((bgInfo.slowestPath ?? bgInfo.slowest_path) as
      Record<string, unknown>[] | undefined) ?? [];
  const rawTopTargets =
    ((bgInfo.topLevelTargets ?? bgInfo.top_level_targets) as
      unknown[] | undefined) ?? [];

  const criticalEntries = rawCriticalPath.map(parseEntry);
  const slowestEntries = rawSlowestPath.map(parseEntry);

  return {
    criticalPath: groupByPhase(criticalEntries),
    slowestPath: groupByPhase(slowestEntries),
    topLevelTargets: parseTopLevelTargets(rawTopTargets),
    totalDurationMs: criticalEntries.reduce((s, e) => s + e.wallDurationMs, 0),
    slowestTotalMs: slowestEntries.reduce((s, e) => s + e.wallDurationMs, 0),
  };
}

export function extractCriticalPathData(
  summaries: import('./event-summary-store').EventSummaryStore,
  getEventData: (s: EventSummary) => Record<string, unknown>,
): CriticalPathData | null {
  for (let i = 0; i < summaries.length; i++) {
    if (
      summaries.getType(i) !== 'instant' ||
      summaries.getEventType(i) !== 'buildGraphInfo'
    )
      continue;
    const evt = summaries.get(i);
    const data = getEventData(evt);
    const instant = data.instant as Record<string, unknown> | undefined;
    if (!instant) continue;
    const bgInfo = (instant.buildGraphInfo ?? instant.build_graph_info) as
      Record<string, unknown> | undefined;
    if (!bgInfo) continue;
    return parseCriticalPathFromBgInfo(bgInfo);
  }
  return null;
}

/**
 * Async variant of `extractCriticalPathData`. Use when you need a guaranteed
 * result for large logs whose chunks may not yet be in the LRU cache —
 * `getEventDataAsync` will await the chunk load. The sync version returns
 * empty for missing chunks and merely fire-and-forgets a load, which causes
 * an O(N) miss-then-skip pass that finds nothing on first call.
 */
export async function extractCriticalPathDataAsync(
  summaries: import('./event-summary-store').EventSummaryStore,
  getEventDataAsync: (s: EventSummary) => Promise<Record<string, unknown>>,
): Promise<CriticalPathData | null> {
  for (let i = 0; i < summaries.length; i++) {
    if (
      summaries.getType(i) !== 'instant' ||
      summaries.getEventType(i) !== 'buildGraphInfo'
    )
      continue;
    const evt = summaries.get(i);
    const data = await getEventDataAsync(evt);
    const instant = data.instant as Record<string, unknown> | undefined;
    if (!instant) continue;
    const bgInfo = (instant.buildGraphInfo ?? instant.build_graph_info) as
      Record<string, unknown> | undefined;
    if (!bgInfo) continue;
    return parseCriticalPathFromBgInfo(bgInfo);
  }
  return null;
}
