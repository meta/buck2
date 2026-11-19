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

import {useState} from 'react';
import {ChevronRightIcon} from 'lucide-react';
import FilterableValue from './FilterableValue';
import type {ActiveFilter} from './filters';
import {hasActiveFieldFilter} from './filters';

interface JsonTreeViewProps {
  data: unknown;
  path?: string;
  depth?: number;
  onFilter: (path: string, value: string) => void;
  activeFilters: ActiveFilter[];
  /** Auto-expand these top-level keys */
  autoExpandKeys?: Set<string>;
}

/** Keys to auto-expand at the top level of an event */
const DEFAULT_AUTO_EXPAND = new Set(['spanStart', 'spanEnd', 'instant']);

export default function JsonTreeView({
  data,
  path = '',
  depth = 0,
  onFilter,
  activeFilters,
  autoExpandKeys = DEFAULT_AUTO_EXPAND,
}: JsonTreeViewProps) {
  if (data === null || data === undefined) {
    return (
      <FilterableValue
        path={path}
        value={null}
        isActive={false}
        onFilter={onFilter}
      />
    );
  }

  if (typeof data !== 'object') {
    return (
      <FilterableValue
        path={path}
        value={data}
        isActive={hasActiveFieldFilter(activeFilters, path, String(data))}
        onFilter={onFilter}
      />
    );
  }

  if (Array.isArray(data)) {
    return (
      <ArrayNode
        data={data}
        path={path}
        depth={depth}
        onFilter={onFilter}
        activeFilters={activeFilters}
      />
    );
  }

  return (
    <ObjectNode
      data={data as Record<string, unknown>}
      path={path}
      depth={depth}
      onFilter={onFilter}
      activeFilters={activeFilters}
      autoExpandKeys={depth === 0 ? autoExpandKeys : undefined}
    />
  );
}

function ObjectNode({
  data,
  path,
  depth,
  onFilter,
  activeFilters,
  autoExpandKeys,
}: {
  data: Record<string, unknown>;
  path: string;
  depth: number;
  onFilter: (path: string, value: string) => void;
  activeFilters: ActiveFilter[];
  autoExpandKeys?: Set<string>;
}) {
  const entries = Object.entries(data);
  if (entries.length === 0)
    return <span className="text-gray-400">{'{}'}</span>;

  return (
    <div className="space-y-0.5">
      {entries.map(([key, value]) => {
        const childPath = path ? `${path}.${key}` : key;
        const isExpandable = value != null && typeof value === 'object';
        const shouldAutoExpand = autoExpandKeys?.has(key) ?? false;

        if (isExpandable) {
          return (
            <CollapsibleEntry
              key={key}
              label={key}
              path={childPath}
              depth={depth}
              defaultOpen
              onFilter={onFilter}
              activeFilters={activeFilters}>
              {value}
            </CollapsibleEntry>
          );
        }

        return (
          <div
            key={key}
            className="flex items-start gap-1"
            style={{paddingLeft: depth > 0 ? 12 : 0}}>
            <span className="text-gray-500 shrink-0 font-medium">{key}:</span>
            <JsonTreeView
              data={value}
              path={childPath}
              depth={depth + 1}
              onFilter={onFilter}
              activeFilters={activeFilters}
            />
          </div>
        );
      })}
    </div>
  );
}

function ArrayNode({
  data,
  path,
  depth,
  onFilter,
  activeFilters,
}: {
  data: unknown[];
  path: string;
  depth: number;
  onFilter: (path: string, value: string) => void;
  activeFilters: ActiveFilter[];
}) {
  const [open, setOpen] = useState(true);

  return (
    <div style={{paddingLeft: depth > 0 ? 0 : undefined}}>
      <button
        onClick={() => setOpen(!open)}
        className="flex items-center gap-0.5 text-gray-500 hover:text-foreground">
        <ChevronRightIcon
          className={`size-3 transition-transform ${open ? 'rotate-90' : ''}`}
        />
        <span>[{data.length} items]</span>
      </button>
      {open && (
        <div className="ml-4 space-y-0.5">
          {data.map((item, i) => (
            <div key={i} className="flex items-start gap-1">
              <span className="text-gray-400 shrink-0">{i}:</span>
              <JsonTreeView
                data={item}
                path={`${path}[${i}]`}
                depth={depth + 1}
                onFilter={onFilter}
                activeFilters={activeFilters}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function CollapsibleEntry({
  label,
  path,
  depth,
  defaultOpen,
  children,
  onFilter,
  activeFilters,
}: {
  label: string;
  path: string;
  depth: number;
  defaultOpen: boolean;
  children: unknown;
  onFilter: (path: string, value: string) => void;
  activeFilters: ActiveFilter[];
}) {
  const [open, setOpen] = useState(defaultOpen);

  const summary = !open ? summarize(children) : null;

  return (
    <div style={{paddingLeft: depth > 0 ? 12 : 0}}>
      <button
        onClick={() => setOpen(!open)}
        className="flex items-center gap-0.5 hover:text-foreground">
        <ChevronRightIcon
          className={`size-3 text-gray-400 transition-transform ${open ? 'rotate-90' : ''}`}
        />
        <span className="font-medium text-gray-500">{label}</span>
        {summary && (
          <span className="text-gray-400 ml-1 truncate max-w-xs">
            {summary}
          </span>
        )}
      </button>
      {open && (
        <div className="ml-2">
          <JsonTreeView
            data={children}
            path={path}
            depth={depth + 1}
            onFilter={onFilter}
            activeFilters={activeFilters}
          />
        </div>
      )}
    </div>
  );
}

function summarize(value: unknown): string {
  if (value == null) return 'null';
  if (Array.isArray(value)) return `[${value.length}]`;
  if (typeof value === 'object') {
    const keys = Object.keys(value);
    if (keys.length <= 3) return `{${keys.join(', ')}}`;
    return `{${keys.slice(0, 3).join(', ')}, ...}`;
  }
  return String(value);
}
