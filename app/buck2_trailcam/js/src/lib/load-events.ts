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
import type {EventSummaryStore} from './event-summary-store';

export interface LoadPackageSpan {
  path: string;
  durationMs: number;
  spanId: number;
  /** Peak starlark heap bytes (from LoadBuildFileEnd) */
  starlarkPeakAllocatedBytes?: number;
  /** CPU instructions (from LoadBuildFileEnd) */
  cpuInstructionCount?: number;
  /** Number of targets declared (from LoadBuildFileEnd) */
  targetCount?: number;
}

export type LoadMetric =
  | 'durationMs'
  | 'starlarkPeakAllocatedBytes'
  | 'cpuInstructionCount'
  | 'targetCount'
  | 'packageCount';

export const LOAD_METRICS: {
  id: LoadMetric;
  label: string;
  format: (v: number) => string;
}[] = [
  {id: 'durationMs', label: 'Duration', format: v => formatMsExport(v)},
  {
    id: 'starlarkPeakAllocatedBytes',
    label: 'Starlark Memory',
    format: formatBytes,
  },
  {id: 'cpuInstructionCount', label: 'CPU Instructions', format: formatCount},
  {id: 'targetCount', label: 'Target Count', format: v => String(v)},
  {id: 'packageCount', label: 'Package Count', format: v => String(v)},
];

function formatMsExport(ms: number): string {
  if (ms < 1) return `${(ms * 1000).toFixed(0)}us`;
  if (ms < 1000) return `${ms.toFixed(1)}ms`;
  return `${(ms / 1000).toFixed(2)}s`;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function formatCount(n: number): string {
  if (n < 1000) return String(n);
  if (n < 1e6) return `${(n / 1000).toFixed(1)}K`;
  return `${(n / 1e6).toFixed(1)}M`;
}

export interface TreemapNodeMetrics {
  durationMs: number;
  starlarkPeakAllocatedBytes: number;
  cpuInstructionCount: number;
  targetCount: number;
  packageCount: number;
}

export interface TreemapNode {
  name: string;
  /** The value used for sizing (set by the selected metric) */
  value: number;
  /** All aggregated metrics for this node */
  metrics: TreemapNodeMetrics;
  children?: TreemapNode[];
  /** Full package path (for tooltips on leaf nodes) */
  fullPath?: string;
  /** Fill color */
  fill?: string;
}

/**
 * Convert a protobuf Duration ({seconds: string, nanos: number}) to milliseconds.
 */
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

/**
 * Extract load spans from decoded events.
 *
 * Merges data from two span types:
 * - `load_package` spans provide the package path and duration
 * - `load` (LoadBuildFile) spans provide starlark memory, CPU instructions, and target count
 *
 * LoadBuildFile spans use `module_id` which matches the `load_package` path.
 */
export function extractLoadPackageSpans(
  summaries: EventSummaryStore,
  getEventData: (s: EventSummary) => Record<string, unknown>,
): LoadPackageSpan[] {
  const startPaths = new Map<number, string>();
  const results: LoadPackageSpan[] = [];
  const buildFileMetrics = new Map<
    string,
    {
      starlarkPeakAllocatedBytes?: number;
      cpuInstructionCount?: number;
      targetCount?: number;
      durationMs?: number;
    }
  >();

  // Iterate using direct accessors; only materialize matching events
  for (let i = 0; i < summaries.length; i++) {
    const eventType = summaries.getEventType(i);
    if (eventType !== 'loadPackage' && eventType !== 'load') continue;
    const spanId = summaries.getSpanId(i);
    if (spanId == null) continue;
    const evt = summaries.get(i);
    const data = getEventData(evt);

    const spanStart = data.spanStart as Record<string, unknown> | undefined;
    if (spanStart?.loadPackage) {
      const lp = spanStart.loadPackage as Record<string, unknown>;
      if (typeof lp.path === 'string') {
        startPaths.set(spanId, lp.path);
      }
      continue;
    }

    const spanEnd = data.spanEnd as Record<string, unknown> | undefined;
    if (!spanEnd) continue;

    if (spanEnd.load) {
      const lbf = spanEnd.load as Record<string, unknown>;
      const rawModuleId = (lbf.moduleId ?? lbf.module_id) as string | undefined;
      if (rawModuleId) {
        const moduleId = rawModuleId.replace(/:[^/]+$/, '');
        const duration = spanEnd.duration as
          Record<string, unknown> | undefined;
        buildFileMetrics.set(moduleId, {
          starlarkPeakAllocatedBytes: asOptionalNumber(
            lbf.starlarkPeakAllocatedBytes ?? lbf.starlark_peak_allocated_bytes,
          ),
          cpuInstructionCount: asOptionalNumber(
            lbf.cpuInstructionCount ?? lbf.cpu_instruction_count,
          ),
          targetCount: asOptionalNumber(lbf.targetCount ?? lbf.target_count),
          durationMs: duration ? durationToMs(duration) : undefined,
        });
      }
    }

    if (spanEnd.loadPackage) {
      const lp = spanEnd.loadPackage as Record<string, unknown>;
      const path =
        startPaths.get(spanId) ??
        (typeof lp.path === 'string' ? lp.path : null);
      const duration = spanEnd.duration as Record<string, unknown> | undefined;

      if (path && duration) {
        results.push({
          path,
          durationMs: durationToMs(duration),
          spanId: spanId,
        });
      }
    }
  }

  // Merge LoadBuildFile metrics onto LoadPackage spans by matching paths
  for (const span of results) {
    const metrics = buildFileMetrics.get(span.path);
    if (metrics) {
      span.starlarkPeakAllocatedBytes = metrics.starlarkPeakAllocatedBytes;
      span.cpuInstructionCount = metrics.cpuInstructionCount;
      span.targetCount = metrics.targetCount;
      // If we got a more specific duration from the build file, prefer it
      // (load_package includes overhead beyond just evaluating the BUCK file)
    }
  }

  return results;
}

function asOptionalNumber(v: unknown): number | undefined {
  if (v == null) return undefined;
  const n = Number(v);
  return isNaN(n) ? undefined : n;
}

/**
 * Parse a package path like "fbcode//some/package/dir" into segments.
 */
function parsePackagePath(path: string): string[] {
  const cellSep = path.indexOf('//');
  if (cellSep < 0) return [path];
  const cell = path.slice(0, cellSep);
  const rest = path.slice(cellSep + 2);
  if (!rest) return [cell];
  return [cell, ...rest.split('/')];
}

interface BuildNode {
  name: string;
  leafMetrics: TreemapNodeMetrics;
  fullPath?: string;
  children: Map<string, BuildNode>;
}

function emptyMetrics(): TreemapNodeMetrics {
  return {
    durationMs: 0,
    starlarkPeakAllocatedBytes: 0,
    cpuInstructionCount: 0,
    targetCount: 0,
    packageCount: 0,
  };
}

function addMetrics(
  a: TreemapNodeMetrics,
  b: TreemapNodeMetrics,
): TreemapNodeMetrics {
  return {
    durationMs: a.durationMs + b.durationMs,
    starlarkPeakAllocatedBytes:
      a.starlarkPeakAllocatedBytes + b.starlarkPeakAllocatedBytes,
    cpuInstructionCount: a.cpuInstructionCount + b.cpuInstructionCount,
    targetCount: a.targetCount + b.targetCount,
    packageCount: a.packageCount + b.packageCount,
  };
}

/**
 * Build a treemap hierarchy from a flat list of package load spans.
 * @param metric Which metric to use for node values (default: durationMs)
 */
export function buildTreemapData(
  packages: LoadPackageSpan[],
  metric: LoadMetric = 'durationMs',
): TreemapNode {
  // Build raw tree with Maps for fast lookup
  const root: BuildNode = {
    name: 'root',
    leafMetrics: emptyMetrics(),
    children: new Map(),
  };

  for (const pkg of packages) {
    const pkgMetrics: TreemapNodeMetrics = {
      durationMs: pkg.durationMs,
      starlarkPeakAllocatedBytes: pkg.starlarkPeakAllocatedBytes ?? 0,
      cpuInstructionCount: pkg.cpuInstructionCount ?? 0,
      targetCount: pkg.targetCount ?? 0,
      packageCount: 1,
    };

    // Skip packages where all metrics are zero
    if (Object.values(pkgMetrics).every(v => v <= 0)) continue;

    const segments = parsePackagePath(pkg.path);
    let node = root;

    // Navigate/create directory nodes
    for (const seg of segments) {
      let child = node.children.get(seg);
      if (!child) {
        child = {name: seg, leafMetrics: emptyMetrics(), children: new Map()};
        node.children.set(seg, child);
      }
      node = child;
    }

    // Add a BUCK leaf node under the package directory
    let buckNode = node.children.get('BUCK');
    if (!buckNode) {
      buckNode = {
        name: 'BUCK',
        leafMetrics: emptyMetrics(),
        children: new Map(),
      };
      node.children.set('BUCK', buckNode);
    }
    buckNode.leafMetrics = addMetrics(buckNode.leafMetrics, pkgMetrics);
    buckNode.fullPath = pkg.path;
  }

  // Convert to TreemapNode, computing summed values and collapsing
  const result = convertNode(root, metric);

  // Assign colors
  colorize(result);

  return result;
}

/**
 * Convert BuildNode tree to TreemapNode tree.
 * Collapses single-child intermediate chains (but not leaves).
 * Node names are local segments (e.g. "buck2" or "buck2/app") — display
 * formatting with cell prefixes (fbcode//) is handled by the component.
 */
function convertNode(node: BuildNode, metric: LoadMetric): TreemapNode {
  // Leaf node (no children)
  if (node.children.size === 0) {
    return {
      name: node.name,
      value: node.leafMetrics[metric],
      metrics: node.leafMetrics,
      fullPath: node.fullPath,
    };
  }

  // Collapse single-child chains for intermediate nodes
  let current = node;
  let collapsedName = node.name;
  const isMetricsEmpty = (m: TreemapNodeMetrics) =>
    m.durationMs === 0 &&
    m.starlarkPeakAllocatedBytes === 0 &&
    m.cpuInstructionCount === 0 &&
    m.targetCount === 0 &&
    m.packageCount === 0;
  while (current.children.size === 1 && isMetricsEmpty(current.leafMetrics)) {
    const only = current.children.values().next().value!;
    if (only.children.size === 0) break;
    collapsedName = `${collapsedName}/${only.name}`;
    current = only;
  }

  const children = Array.from(current.children.values()).map(c =>
    convertNode(c, metric),
  );
  let combinedMetrics = current.leafMetrics;
  for (const c of children) {
    combinedMetrics = addMetrics(combinedMetrics, c.metrics);
  }

  return {
    name: collapsedName,
    value: combinedMetrics[metric],
    metrics: combinedMetrics,
    children,
    fullPath: current.fullPath,
  };
}

/**
 * Assign fill colors to nodes based on value relative to max.
 * Warm (red/orange) = slow, cool (blue/teal) = fast.
 */
function colorize(root: TreemapNode): void {
  const maxVal = findMaxLeaf(root);
  if (maxVal === 0) return;
  assignColors(root, maxVal);
}

function findMaxLeaf(node: TreemapNode): number {
  if (!node.children?.length) return node.value;
  let max = 0;
  for (const c of node.children) {
    max = Math.max(max, findMaxLeaf(c));
  }
  return max;
}

function assignColors(node: TreemapNode, maxVal: number): void {
  // Color all nodes (both leaves and intermediates)
  const ratio = Math.min(node.value / maxVal, 1);
  // HSL: 200 (blue) -> 0 (red) as ratio goes 0 -> 1
  const hue = Math.round(200 * (1 - ratio));
  const sat = 50 + Math.round(20 * ratio);
  const light = 40 + Math.round(15 * (1 - ratio));
  node.fill = `hsl(${hue}, ${sat}%, ${light}%)`;

  for (const c of node.children ?? []) {
    assignColors(c, maxVal);
  }
}
