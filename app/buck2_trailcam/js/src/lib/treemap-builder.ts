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
 * Generic treemap builder: takes items with a path and a set of numeric metrics,
 * builds a hierarchical tree grouped by path segments.
 */

export interface TreemapMetrics {
  [key: string]: number;
}

export interface TreemapNode<M extends TreemapMetrics = TreemapMetrics> {
  name: string;
  value: number;
  metrics: M;
  children?: TreemapNode<M>[];
  fullPath?: string;
  fill?: string;
}

export interface TreemapItem<M extends TreemapMetrics> {
  /** Path to group by, e.g. "fbcode//foo/bar" */
  path: string;
  /** Metrics for this item */
  metrics: M;
}

interface BuildNode<M extends TreemapMetrics> {
  name: string;
  leafMetrics: M;
  fullPath?: string;
  children: Map<string, BuildNode<M>>;
}

/**
 * Parse a package path like "fbcode//some/package/dir" into segments.
 */
function parsePackagePath(path: string): string[] {
  const cellSep = path.indexOf('//');
  if (cellSep < 0) return path.split('/').filter(Boolean);
  const cell = path.slice(0, cellSep);
  const rest = path.slice(cellSep + 2);
  if (!rest) return [cell];
  return [cell, ...rest.split('/')];
}

function emptyMetrics<M extends TreemapMetrics>(template: M): M {
  const result: TreemapMetrics = {};
  for (const key of Object.keys(template)) {
    result[key] = 0;
  }
  return result as M;
}

function addMetrics<M extends TreemapMetrics>(a: M, b: M): M {
  const result: TreemapMetrics = {};
  for (const key of Object.keys(a)) {
    result[key] = (a[key] ?? 0) + (b[key] ?? 0);
  }
  return result as M;
}

/**
 * Build a treemap from items grouped by path segments.
 *
 * @param items - Items with a path and metrics
 * @param metricKey - Which metric key to use for the node `value` (determines sizing)
 * @param leafName - Optional name for leaf nodes (e.g. "BUCK"). If provided, adds
 *                   a child node with this name under each package directory.
 */
export function buildTreemap<M extends TreemapMetrics>(
  items: TreemapItem<M>[],
  metricKey: string,
  leafName?: string,
): TreemapNode<M> {
  if (items.length === 0) {
    const empty = emptyMetrics(items[0]?.metrics ?? ({} as M));
    return {name: 'root', value: 0, metrics: empty};
  }

  const template = emptyMetrics(items[0].metrics);
  const root: BuildNode<M> = {
    name: 'root',
    leafMetrics: template,
    children: new Map(),
  };

  for (const item of items) {
    // Skip items where all metrics are zero
    if (Object.values(item.metrics).every(v => v <= 0)) continue;

    const segments = parsePackagePath(item.path);
    let node = root;

    for (const seg of segments) {
      let child = node.children.get(seg);
      if (!child) {
        child = {
          name: seg,
          leafMetrics: emptyMetrics(template),
          children: new Map(),
        };
        node.children.set(seg, child);
      }
      node = child;
    }

    if (leafName) {
      // Add a named leaf node under the package directory
      let leaf = node.children.get(leafName);
      if (!leaf) {
        leaf = {
          name: leafName,
          leafMetrics: emptyMetrics(template),
          children: new Map(),
        };
        node.children.set(leafName, leaf);
      }
      leaf.leafMetrics = addMetrics(leaf.leafMetrics, item.metrics);
      leaf.fullPath = item.path;
    } else {
      // Put metrics directly on the package node
      node.leafMetrics = addMetrics(node.leafMetrics, item.metrics);
      node.fullPath = item.path;
    }
  }

  const result = convertNode(root, metricKey, template);
  colorize(result);
  return result;
}

function convertNode<M extends TreemapMetrics>(
  node: BuildNode<M>,
  metricKey: string,
  template: M,
): TreemapNode<M> {
  if (node.children.size === 0) {
    return {
      name: node.name,
      value: (node.leafMetrics as TreemapMetrics)[metricKey] ?? 0,
      metrics: node.leafMetrics,
      fullPath: node.fullPath,
    };
  }

  // Collapse single-child chains for intermediate nodes
  let current = node;
  let collapsedName = node.name;
  const isEmpty = (m: M) => Object.values(m).every(v => v === 0);
  while (current.children.size === 1 && isEmpty(current.leafMetrics)) {
    const only = current.children.values().next().value!;
    if (only.children.size === 0) break;
    collapsedName = `${collapsedName}/${only.name}`;
    current = only;
  }

  const children = Array.from(current.children.values()).map(c =>
    convertNode(c, metricKey, template),
  );
  let combinedMetrics = current.leafMetrics;
  for (const c of children) {
    combinedMetrics = addMetrics(combinedMetrics, c.metrics);
  }

  return {
    name: collapsedName,
    value: (combinedMetrics as TreemapMetrics)[metricKey] ?? 0,
    metrics: combinedMetrics,
    children,
    fullPath: current.fullPath,
  };
}

function colorize<M extends TreemapMetrics>(root: TreemapNode<M>): void {
  const maxVal = findMaxLeaf(root);
  if (maxVal === 0) return;
  assignColors(root, maxVal);
}

function findMaxLeaf<M extends TreemapMetrics>(node: TreemapNode<M>): number {
  if (!node.children?.length) return node.value;
  let max = 0;
  for (const c of node.children) {
    max = Math.max(max, findMaxLeaf(c));
  }
  return max;
}

function assignColors<M extends TreemapMetrics>(
  node: TreemapNode<M>,
  maxVal: number,
): void {
  const ratio = Math.min(node.value / maxVal, 1);
  const hue = Math.round(200 * (1 - ratio));
  const sat = 50 + Math.round(20 * ratio);
  const light = 40 + Math.round(15 * (1 - ratio));
  node.fill = `hsl(${hue}, ${sat}%, ${light}%)`;
  for (const c of node.children ?? []) {
    assignColors(c, maxVal);
  }
}
