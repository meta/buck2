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

import {useMemo} from 'react';
import {useEventLog, useLazyAggregate} from './EventLogProvider';
import {
  extractActionSpans,
  ACTION_METRICS,
  type ActionSpan,
} from '../../lib/action-events';
import {buildTreemap} from '../../lib/treemap-builder';
import GenericTreemapView from './GenericTreemapView';
import {useUrlState} from '../../lib/url-state';

export default function ActionTreemap() {
  const logState = useEventLog();
  const lazy = useLazyAggregate('actionSpans');
  const [metric, setMetric] = useUrlState<string>('m', 'durationMs', {
    parse: s => (ACTION_METRICS.some(mm => mm.id === s) ? s : 'durationMs'),
  });

  const spans: ActionSpan[] | null = useMemo(() => {
    if (logState.status !== 'loaded') return null;
    if (lazy.data && lazy.data.length > 0) return lazy.data;
    if (lazy.loading) return null;
    // Action span extraction on the main thread is expensive for large logs
    // (iterates all events, decompressing IDB chunks for each). The worker
    // disables the action collector for large logs to keep memory usage down,
    // and we don't want to compute it on the main thread either.
    if (logState.isLargeLog) return null;
    return extractActionSpans(logState.summaries, logState.getEventData);
  }, [logState, lazy]);

  const treemapData = useMemo(() => {
    if (!spans || spans.length === 0) return null;
    const items = spans.map(s => ({
      path: s.treemapPath,
      metrics: {
        durationMs: s.durationMs,
        wallTimeMs: s.wallTimeMs,
        outputSizeBytes: s.outputSizeBytes,
        inputFilesSizeBytes: s.inputFilesSizeBytes,
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
              ? 'Loading action spans...'
              : 'Loading...'}
        </p>
      </div>
    );
  }

  if (logState.status === 'error' || !treemapData) {
    const reason =
      logState.status === 'error'
        ? logState.message
        : logState.status === 'loaded' && logState.isLargeLog && spans === null
          ? 'Action treemap is disabled for large logs (>100MB compressed) to avoid expensive recomputation on the main thread.'
          : 'No action execution data available.';
    return (
      <div className="text-muted-foreground rounded border border-dashed p-12 text-center">
        <p className="text-lg font-medium">Actions</p>
        <p className="mt-1 text-sm">{reason}</p>
      </div>
    );
  }

  return (
    <GenericTreemapView
      treemapData={treemapData}
      metrics={ACTION_METRICS}
      activeMetric={metric}
      onMetricChange={m => setMetric(m)}
      itemCount={spans?.length ?? 0}
      itemLabel="actions"
    />
  );
}
