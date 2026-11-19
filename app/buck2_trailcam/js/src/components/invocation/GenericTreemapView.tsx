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

import {useState, useEffect, useCallback, useRef} from 'react';
import {Treemap, ResponsiveContainer} from 'recharts';
import type {TreemapNode, TreemapMetrics} from '../../lib/treemap-builder';

interface MetricDef {
  id: string;
  label: string;
  format: (v: number) => string;
}

interface TooltipData {
  displayPath: string;
  value: number;
  total: number;
  hasChildren: boolean;
  metrics?: TreemapMetrics;
  x: number;
  y: number;
}

// Module-level state read by TreemapCell (recharts doesn't pass custom props)
let activeMetricId: string = 'durationMs';
let activeMetrics: MetricDef[] = [];

function buildDisplayPath(segments: string[]): string {
  if (segments.length === 0) return '';
  if (segments.length === 1) return `${segments[0]}//`;
  return `${segments[0]}//${segments.slice(1).join('/')}/`;
}

function resolveSubtree<M extends TreemapMetrics>(
  root: TreemapNode<M>,
  path: string[],
): TreemapNode<M> {
  let node = root;
  for (const seg of path) {
    const child = node.children?.find(c => c.name === seg);
    if (!child) break;
    node = child;
  }
  return node;
}

function TreemapCell(props: Record<string, unknown>) {
  const {x, y, width, height, displayPath, fill, depth, metrics} = props as {
    x: number;
    y: number;
    width: number;
    height: number;
    displayPath?: string;
    fill: string;
    depth: number;
    metrics?: TreemapMetrics;
  };

  if (width < 2 || height < 2) return null;

  const label = displayPath ?? '';
  const charWidth = 8;
  const maxChars = Math.floor((width - 8) / charWidth);
  const truncatedName =
    maxChars > 0 && label.length > maxChars
      ? label.slice(0, maxChars - 1) + '\u2026'
      : label;

  const metricLines: {text: string; bold: boolean}[] = [];
  if (metrics) {
    for (const m of activeMetrics) {
      const val = metrics[m.id] ?? 0;
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

export default function GenericTreemapView<M extends TreemapMetrics>({
  treemapData,
  metrics,
  activeMetric,
  onMetricChange,
  itemCount,
  itemLabel,
}: {
  treemapData: TreemapNode<M>;
  metrics: MetricDef[];
  activeMetric: string;
  onMetricChange: (metric: string) => void;
  itemCount: number;
  itemLabel: string;
}) {
  const [drillPath, setDrillPath] = useState<string[]>([]);
  const [tooltip, setTooltip] = useState<TooltipData | null>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const [containerHeight, setContainerHeight] = useState(500);
  const mousePos = useRef({x: 0, y: 0});

  // Set module-level state for TreemapCell
  activeMetricId = activeMetric;
  activeMetrics = metrics;

  const metricInfo = metrics.find(m => m.id === activeMetric)!;
  const formatValue = metricInfo.format;

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
  }, []);

  const currentNode = resolveSubtree(treemapData, drillPath);
  const total = treemapData.metrics[activeMetric] ?? 0;

  // Build path segments from drillPath
  const pathSegments: string[] = [];
  for (const name of drillPath) {
    pathSegments.push(...name.split('/'));
  }

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

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    mousePos.current = {x: e.clientX, y: e.clientY};
    setTooltip(prev => (prev ? {...prev, x: e.clientX, y: e.clientY} : null));
  }, []);

  const handleMouseEnter = useCallback(
    (node: Record<string, unknown>) => {
      setTooltip({
        displayPath: (node.displayPath ?? node.name) as string,
        value: node.value as number,
        total,
        hasChildren: !!(node.hasChildren as boolean),
        metrics: node.metrics as TreemapMetrics,
        x: mousePos.current.x,
        y: mousePos.current.y,
      });
    },
    [total],
  );

  const handleMouseLeave = useCallback(() => setTooltip(null), []);

  const handleClick = useCallback((node: Record<string, unknown>) => {
    if (node.hasChildren) {
      setDrillPath(prev => [...prev, node.name as string]);
    }
  }, []);

  return (
    <div>
      {/* Metric selector */}
      <div className="mb-2 flex items-center gap-3">
        <div className="flex shrink-0 rounded border">
          {metrics.map(m => (
            <button
              key={m.id}
              onClick={() => {
                onMetricChange(m.id);
                setDrillPath([]);
              }}
              className={`px-2.5 py-1 text-xs transition-colors ${
                activeMetric === m.id
                  ? 'bg-gray-900 text-white dark:bg-gray-100 dark:text-gray-900'
                  : 'text-muted-foreground hover:bg-gray-100 dark:hover:bg-gray-800'
              } first:rounded-l last:rounded-r`}>
              {m.label}
            </button>
          ))}
        </div>
      </div>

      {/* Breadcrumb + stats */}
      <div className="mb-2 flex items-baseline justify-between">
        <div className="flex items-baseline text-sm">
          <button
            onClick={() => setDrillPath([])}
            className={`hover:underline ${drillPath.length === 0 ? 'font-semibold' : 'text-blue-600 dark:text-blue-400'}`}>
            All
          </button>
          {pathSegments.map((seg, i) => {
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
        <span className="text-muted-foreground text-xs">
          {itemCount} {itemLabel} &middot; {formatValue(total)} total
        </span>
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
            <p className="font-mono font-semibold">{tooltip.displayPath}</p>
            {tooltip.metrics && (
              <div className="mt-1.5 space-y-0.5">
                {metrics.map(m => {
                  const val = tooltip.metrics![m.id] ?? 0;
                  if (!val) return null;
                  const isSelected = m.id === activeMetric;
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
            {tooltip.hasChildren && (
              <p className="mt-1 text-xs text-blue-300">Click to drill down</p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
