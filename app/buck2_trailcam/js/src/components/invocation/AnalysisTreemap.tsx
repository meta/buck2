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

import {useState, useMemo, useCallback, useRef} from 'react';
import {Treemap, ResponsiveContainer} from 'recharts';
import {useEventLog, useLazyAggregate} from './EventLogProvider';
import {
  extractAnalysisSpans,
  ANALYSIS_METRICS,
  type AnalysisMetric,
  type AnalysisSpan,
} from '../../lib/analysis-events';
import {buildTreemap, type TreemapNode} from '../../lib/treemap-builder';
import GenericTreemapView from './GenericTreemapView';
import {useUrlState} from '../../lib/url-state';

export default function AnalysisTreemap() {
  const logState = useEventLog();
  const lazy = useLazyAggregate('analysisSpans');
  const [metric, setMetric] = useUrlState<string>('m', 'durationMs', {
    parse: s => (ANALYSIS_METRICS.some(m => m.id === s) ? s : 'durationMs'),
  });

  const spans: AnalysisSpan[] | null = useMemo(() => {
    if (logState.status !== 'loaded') return null;
    if (lazy.data && lazy.data.length > 0) return lazy.data;
    if (lazy.loading) return null;
    return extractAnalysisSpans(logState.summaries, logState.getEventData);
  }, [logState, lazy]);

  const treemapData = useMemo(() => {
    if (!spans || spans.length === 0) return null;
    const items = spans.map(s => ({
      path: s.packagePath,
      metrics: {
        durationMs: s.durationMs,
        declaredActions: s.declaredActions,
        declaredArtifacts: s.declaredArtifacts,
        retainedMemoryBytes: s.retainedMemoryBytes,
        count: 1,
      },
    }));
    return buildTreemap(items, metric);
  }, [spans, metric]);

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
              ? 'Loading analysis spans...'
              : 'Loading...'}
        </p>
      </div>
    );
  }

  if (logState.status === 'error' || !treemapData) {
    return (
      <div className="text-muted-foreground rounded border border-dashed p-12 text-center">
        <p className="text-lg font-medium">Analysis</p>
        <p className="mt-1 text-sm">
          {logState.status === 'error'
            ? logState.message
            : 'No analysis data available.'}
        </p>
      </div>
    );
  }

  return (
    <GenericTreemapView
      treemapData={treemapData}
      metrics={ANALYSIS_METRICS}
      activeMetric={metric}
      onMetricChange={m => setMetric(m)}
      itemCount={spans?.length ?? 0}
      itemLabel="targets"
    />
  );
}
