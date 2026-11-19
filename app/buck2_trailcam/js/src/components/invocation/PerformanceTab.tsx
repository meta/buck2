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

import LoadTreemap from './LoadTreemap';
import AnalysisTreemap from './AnalysisTreemap';
import ActionTreemap from './ActionTreemap';
import CriticalPathView from './CriticalPathView';
import {useUrlState} from '../../lib/url-state';

const subTabs = [
  {id: 'load', label: 'Load'},
  {id: 'analysis', label: 'Analysis'},
  {id: 'actions', label: 'Actions'},
  {id: 'cache', label: 'Cache'},
  {id: 'critical-path', label: 'Critical Path'},
  {id: 're', label: 'Remote Execution'},
] as const;

type SubTabId = (typeof subTabs)[number]['id'];

function Placeholder({
  title,
  description,
}: {
  title: string;
  description: string;
}) {
  return (
    <div className="text-muted-foreground rounded border border-dashed p-12 text-center">
      <p className="text-lg font-medium">{title}</p>
      <p className="mt-1 text-sm">{description}</p>
    </div>
  );
}

export default function PerformanceTab({buildUuid}: {buildUuid: string}) {
  const [activeTab, setActiveTabRaw] = useUrlState<SubTabId>('sub', 'load', {
    parse: s => (subTabs.some(t => t.id === s) ? (s as SubTabId) : 'load'),
  });
  const setActiveTab = (id: SubTabId) => setActiveTabRaw(id);

  return (
    <div className="flex gap-4">
      {/* Vertical sub-tabs */}
      <nav className="flex w-40 shrink-0 flex-col gap-0.5">
        {subTabs.map(tab => (
          <button
            key={tab.id}
            onClick={() => setActiveTab(tab.id)}
            className={`rounded-md px-3 py-1.5 text-left text-sm transition-colors ${
              activeTab === tab.id
                ? 'bg-gray-100 font-medium dark:bg-gray-800'
                : 'text-muted-foreground hover:bg-gray-50 dark:hover:bg-gray-900'
            }`}>
            {tab.label}
          </button>
        ))}
      </nav>

      {/* Sub-tab content */}
      <div className="min-w-0 flex-1">
        {activeTab === 'load' && <LoadTreemap />}
        {activeTab === 'analysis' && <AnalysisTreemap />}
        {activeTab === 'actions' && <ActionTreemap />}
        {activeTab === 'cache' && (
          <Placeholder
            title="Cache"
            description="Cache hit/miss analysis: hit rates by rule type, cache check latency, upload stats."
          />
        )}
        {activeTab === 'critical-path' && <CriticalPathView />}
        {activeTab === 're' && (
          <Placeholder
            title="Remote Execution"
            description="RE session details: queue times, execution times, upload/download throughput."
          />
        )}
      </div>
    </div>
  );
}
