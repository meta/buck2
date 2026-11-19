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

import {useState, useEffect, useCallback, useMemo, useRef} from 'react';
import {Treemap, ResponsiveContainer} from 'recharts';
import {useEventLog, useLazyAggregate} from './EventLogProvider';
import {
  extractLoadPackageSpans,
  buildTreemapData,
  LOAD_METRICS,
  type LoadPackageSpan,
  type TreemapNode,
  type LoadMetric,
} from '../../lib/load-events';
import {useUrlState} from '../../lib/url-state';

// Module-level state for TreemapCell (set by the component, read by the cell
// which can't receive props directly from recharts)
let activeMetricId: LoadMetric = 'durationMs';

interface TooltipData {
  name: string;
  fullPath?: string;
  value: number;
  x: number;
  y: number;
  total: number;
  hasChildren: boolean;
  metrics?: {
    durationMs: number;
    starlarkPeakAllocatedBytes: number;
    cpuInstructionCount: number;
    targetCount: number;
    packageCount: number;
  };
}

/**
 * Build a display path from segments, using // after the first (cell) segment.
 * e.g. ["fbcode", "buck2", "app"] => "fbcode//buck2/app/"
 * The cell segment alone renders as "fbcode//"
 */
function buildDisplayPath(segments: string[]): string {
  if (segments.length === 0) return '';
  if (segments.length === 1) return `${segments[0]}//`;
  return `${segments[0]}//${segments.slice(1).join('/')}/`;
}

/**
 * Walk the treemap tree following the drill path to find the current subtree.
 */
function resolveSubtree(root: TreemapNode, path: string[]): TreemapNode {
  let node = root;
  for (const seg of path) {
    const child = node.children?.find(c => c.name === seg);
    if (!child) break;
    node = child;
  }
  return node;
}

// Custom content renderer for treemap cells
function TreemapCell(props: Record<string, unknown>) {
  const {x, y, width, height, displayPath, fill, depth, metrics} = props as {
    x: number;
    y: number;
    width: number;
    height: number;
    displayPath: string;
    fill: string;
    depth: number;
    metrics?: {
      durationMs: number;
      starlarkPeakAllocatedBytes: number;
      cpuInstructionCount: number;
      targetCount: number;
      packageCount: number;
    };
  };

  if (width < 2 || height < 2) return null;

  const label = displayPath ?? '';
  const charWidth = 8;
  const maxChars = Math.floor((width - 8) / charWidth);
  const truncatedName =
    maxChars > 0 && label.length > maxChars
      ? label.slice(0, maxChars - 1) + '\u2026'
      : label;

  // Build metric lines to display
  const metricLines: {text: string; bold: boolean}[] = [];
  if (metrics) {
    for (const m of LOAD_METRICS) {
      const val = metrics[m.id];
      if (val > 0) {
        metricLines.push({
          text: `${m.label}: ${m.format(val)}`,
          bold: m.id === activeMetricId,
        });
      }
    }
  }

  return (
    <g style={{cursor: 'pointer'}}>
      {/* Background rect */}
      <rect
        x={x}
        y={y}
        width={width}
        height={height}
        fill={fill ?? '#6b7280'}
        stroke="#fff"
        strokeWidth={depth === 0 ? 2 : 1}
        style={{filter: 'brightness(1)', transition: 'filter 100ms'}}
        onMouseEnter={e => {
          e.currentTarget.style.filter = 'brightness(1.25)';
        }}
        onMouseLeave={e => {
          e.currentTarget.style.filter = 'brightness(1)';
        }}
      />
      {/* Name label */}
      {width > 30 && height > 18 && (
        <text
          x={x + 6}
          y={y + 18}
          fill="#fff"
          fontSize={15}
          fontWeight={600}
          style={{
            pointerEvents: 'none',
            textShadow: '0 1px 3px rgba(0,0,0,0.6)',
          }}>
          {truncatedName}
        </text>
      )}
      {/* Metric lines */}
      {width > 60 &&
        metricLines.map((line, i) => {
          const lineY = y + 34 + i * 15;
          if (lineY + 10 > y + height) return null;
          return (
            <text
              key={i}
              x={x + 6}
              y={lineY}
              fill={
                line.bold ? 'rgba(255,255,255,0.95)' : 'rgba(255,255,255,0.6)'
              }
              fontSize={11}
              fontWeight={line.bold ? 600 : 400}
              style={{
                pointerEvents: 'none',
                textShadow: '0 1px 2px rgba(0,0,0,0.4)',
              }}>
              {line.text}
            </text>
          );
        })}
    </g>
  );
}

export default function LoadTreemap() {
  const logState = useEventLog();
  const lazy = useLazyAggregate('loadSpans');
  const [tooltip, setTooltip] = useState<TooltipData | null>(null);
  const [drillPath, setDrillPath] = useState<string[]>([]);
  const [metric, setMetric] = useUrlState<LoadMetric>('m', 'durationMs', {
    parse: s =>
      LOAD_METRICS.some(m => m.id === s) ? (s as LoadMetric) : 'durationMs',
  });
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerHeight, setContainerHeight] = useState(500);

  // Use pre-computed aggregates for large logs, on-demand extraction for small logs
  const spans: LoadPackageSpan[] | null = useMemo(() => {
    if (logState.status !== 'loaded') return null;
    if (lazy.data && lazy.data.length > 0) return lazy.data;
    if (lazy.loading) return null;
    return extractLoadPackageSpans(logState.summaries, logState.getEventData);
  }, [logState, lazy]);

  // Build treemap for selected metric
  const treemapResult = useMemo(() => {
    if (!spans) return null;
    const treemapData = buildTreemapData(spans, metric);
    // Use the root node's metrics for the total — it's the sum of all children
    const total = treemapData.metrics[metric];
    return {treemapData, total, packageCount: spans.length};
  }, [spans, metric]);

  const metricInfo = LOAD_METRICS.find(m => m.id === metric)!;
  const formatValue = metricInfo.format;

  // Measure available height to fit within viewport
  useEffect(() => {
    function measure() {
      if (containerRef.current) {
        const rect = containerRef.current.getBoundingClientRect();
        const available = window.innerHeight - rect.top - 16;
        setContainerHeight(Math.max(300, Math.min(1200, available)));
      }
    }
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
  }, [logState.status]);

  const total = treemapResult?.total ?? 0;
  // Set module-level metric so TreemapCell can read it
  activeMetricId = metric;

  const mousePos = useRef({x: 0, y: 0});

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    mousePos.current = {x: e.clientX, y: e.clientY};
    setTooltip(prev => (prev ? {...prev, x: e.clientX, y: e.clientY} : null));
  }, []);

  const handleMouseEnter = useCallback(
    (node: Record<string, unknown>) => {
      setTooltip({
        name: (node.displayPath ?? node.name) as string,
        fullPath: node.fullPath as string | undefined,
        value: node.value as number,
        x: mousePos.current.x,
        y: mousePos.current.y,
        total,
        hasChildren: !!(node.hasChildren as boolean),
        metrics: node.metrics as TooltipData['metrics'],
      });
    },
    [total],
  );

  const handleMouseLeave = useCallback(() => {
    setTooltip(null);
  }, []);

  const handleClick = useCallback(
    (node: Record<string, unknown>) => {
      if (logState.status !== 'loaded') return;
      if (node.hasChildren) {
        setDrillPath(prev => [...prev, node.name as string]);
      }
    },
    [logState.status],
  );

  if (
    logState.status === 'idle' ||
    logState.status === 'loading' ||
    lazy.loading
  ) {
    return (
      <div className="text-muted-foreground flex items-center justify-center py-20">
        <p className="text-sm">
          {logState.status === 'loading'
            ? logState.progress
            : lazy.loading
              ? 'Loading load spans...'
              : 'Loading...'}
        </p>
      </div>
    );
  }

  if (logState.status === 'error' || !treemapResult) {
    return (
      <div className="rounded border border-red-200 bg-red-50 p-4 dark:border-red-800 dark:bg-red-950">
        <p className="text-sm text-red-700 dark:text-red-300">
          {logState.status === 'error'
            ? logState.message
            : 'Failed to process events'}
        </p>
      </div>
    );
  }

  const {treemapData, packageCount} = treemapResult;
  const currentNode = resolveSubtree(treemapData, drillPath);

  // Build the accumulated path segments from the drill path.
  // Each drillPath entry is a node name which may contain collapsed segments (e.g. "buck2/app").
  const pathSegments: string[] = [];
  for (const name of drillPath) {
    pathSegments.push(...name.split('/'));
  }

  // Present immediate children as flat items with display paths
  const displayData = (currentNode.children ?? []).map(c => {
    const childSegments = [...pathSegments, ...c.name.split('/')];
    return {
      name: c.name,
      displayPath: buildDisplayPath(childSegments),
      value: c.value,
      fullPath: c.fullPath,
      fill: c.fill,
      hasChildren: !!c.children?.length,
      metrics: c.metrics,
    };
  });

  return (
    <div>
      {/* Metric selector + breadcrumb + stats */}
      <div className="mb-2 flex items-center gap-3">
        <div className="flex shrink-0 rounded border">
          {LOAD_METRICS.map(m => (
            <button
              key={m.id}
              onClick={() => {
                setMetric(m.id);
                setDrillPath([]);
              }}
              className={`px-2.5 py-1 text-xs transition-colors ${
                metric === m.id
                  ? 'bg-gray-900 text-white dark:bg-gray-100 dark:text-gray-900'
                  : 'text-muted-foreground hover:bg-gray-100 dark:hover:bg-gray-800'
              } first:rounded-l last:rounded-r`}>
              {m.label}
            </button>
          ))}
        </div>
      </div>
      <div className="mb-2 flex items-baseline justify-between">
        <div className="flex items-baseline text-sm">
          <button
            onClick={() => setDrillPath([])}
            className={`hover:underline ${drillPath.length === 0 ? 'font-semibold' : 'text-blue-600 dark:text-blue-400'}`}>
            All
          </button>
          {pathSegments.map((seg, i) => {
            // Figure out which drillPath entry this segment belongs to,
            // so clicking navigates to the right level
            let segCount = 0;
            let drillIdx = 0;
            for (let d = 0; d < drillPath.length; d++) {
              segCount += drillPath[d].split('/').length;
              if (i < segCount) {
                drillIdx = d + 1;
                break;
              }
            }
            const isLast = i === pathSegments.length - 1;
            // First segment is a cell: separator is "//"
            const separator = i === 0 ? '//' : '/';
            return (
              <span key={i} className="flex items-baseline">
                <span className="text-muted-foreground">
                  {i === 0 ? '/' : ''}
                </span>
                <button
                  onClick={() => setDrillPath(drillPath.slice(0, drillIdx))}
                  className={`hover:underline ${isLast ? 'font-semibold' : 'text-blue-600 dark:text-blue-400'}`}>
                  {seg}
                </button>
                <span className="text-muted-foreground">
                  {isLast && i > 0 ? '/' : separator}
                </span>
              </span>
            );
          })}
          <span className="text-muted-foreground ml-2 text-xs">
            {formatValue(currentNode.value)}
            {drillPath.length > 0 && total > 0 && (
              <> ({((currentNode.value / total) * 100).toFixed(1)}% of total)</>
            )}
          </span>
        </div>
        <div className="text-muted-foreground text-xs">
          {packageCount} packages &middot; {formatValue(total)} total
        </div>
      </div>

      {/* Treemap */}
      <div
        ref={containerRef}
        className="relative"
        style={{height: containerHeight, maxWidth: 1800}}
        onMouseMove={handleMouseMove}>
        {displayData.length > 0 ? (
          <ResponsiveContainer width="100%" height="100%">
            <Treemap
              data={displayData as any}
              dataKey="value"
              nameKey="name"
              content={<TreemapCell />}
              onMouseEnter={handleMouseEnter as any}
              onMouseLeave={handleMouseLeave}
              onClick={handleClick as any}
              isAnimationActive={false}
            />
          </ResponsiveContainer>
        ) : (
          <div className="text-muted-foreground flex h-full items-center justify-center text-sm">
            Leaf node — no children to display
          </div>
        )}

        {tooltip && (
          <div
            className="pointer-events-none fixed z-50 max-w-md rounded-lg bg-gray-900 px-4 py-3 text-sm text-white shadow-xl"
            style={{left: tooltip.x + 16, top: tooltip.y - 12}}>
            <p className="font-mono font-semibold">
              {tooltip.fullPath ?? tooltip.name}
            </p>
            {tooltip.metrics && (
              <div className="mt-1.5 space-y-0.5">
                {LOAD_METRICS.map(m => {
                  const val = tooltip.metrics![m.id];
                  if (!val) return null;
                  const isSelected = m.id === metric;
                  return (
                    <p
                      key={m.id}
                      className={
                        isSelected ? 'text-white font-medium' : 'text-gray-400'
                      }>
                      {m.label}: {m.format(val)}
                      {isSelected &&
                        tooltip.total > 0 &&
                        ` (${((val / tooltip.total) * 100).toFixed(1)}%)`}
                    </p>
                  );
                })}
              </div>
            )}
            {!tooltip.metrics && (
              <p className="mt-1 text-gray-300">
                {formatValue(tooltip.value)}
                {tooltip.total > 0 &&
                  ` (${((tooltip.value / tooltip.total) * 100).toFixed(1)}% of total)`}
              </p>
            )}
            {tooltip.hasChildren && (
              <p className="mt-1 text-xs text-blue-300">Click to drill down</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
