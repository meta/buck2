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

import {Card, CardContent, CardHeader, CardTitle, Badge} from '../../ui';
import {formatBytes} from '../../lib/format';
import type {InvocationMetrics} from '../../invocation';

interface Warning {
  title: string;
  description: string;
  severity: 'warning' | 'error' | 'info';
}

function computeWarnings(metrics: InvocationMetrics): Warning[] {
  const warnings: Warning[] = [];

  if (
    metrics.peakUsedDiskSpaceBytes != null &&
    metrics.totalDiskSpaceBytes != null
  ) {
    const used = metrics.peakUsedDiskSpaceBytes;
    const total = metrics.totalDiskSpaceBytes;
    if (total > 0 && used / total > 0.9) {
      warnings.push({
        title: 'Low Disk Space',
        description: `Disk was ${((used / total) * 100).toFixed(1)}% full (${formatBytes(used)} / ${formatBytes(total)}). Low disk space can significantly slow builds.`,
        severity: used / total > 0.95 ? 'error' : 'warning',
      });
    }
  }

  if (
    metrics.peakProcessMemoryBytes != null &&
    metrics.systemTotalMemoryBytes != null
  ) {
    const procMem = metrics.peakProcessMemoryBytes;
    const totalMem = metrics.systemTotalMemoryBytes;
    if (totalMem > 0 && procMem / totalMem > 0.8) {
      warnings.push({
        title: 'High Memory Usage',
        description: `Buck2 used ${formatBytes(procMem)} of ${formatBytes(totalMem)} system memory (${((procMem / totalMem) * 100).toFixed(1)}%). High memory usage can cause slowdowns.`,
        severity: 'warning',
      });
    }
  }

  const changeCount = metrics.fileChangesSinceLastBuildCount ?? 0;
  if (changeCount > 100) {
    warnings.push({
      title: 'Many File Changes',
      description: `${changeCount} files changed since last build. Large change sets reduce cache effectiveness.`,
      severity: 'info',
    });
  }

  if (metrics.concurrentCommandIds.length > 0) {
    warnings.push({
      title: 'Concurrent Commands',
      description: `${metrics.concurrentCommandIds.length} other buck2 command(s) were running concurrently, which can slow builds.`,
      severity: 'info',
    });
  }

  return warnings;
}

const severityStyles: Record<string, string> = {
  error: 'border-red-200 bg-red-50 dark:border-red-800 dark:bg-red-950',
  warning:
    'border-yellow-200 bg-yellow-50 dark:border-yellow-800 dark:bg-yellow-950',
  info: 'border-blue-200 bg-blue-50 dark:border-blue-800 dark:bg-blue-950',
};

export default function SpeedDoctor({
  metrics,
}: {
  metrics: InvocationMetrics | null;
}) {
  if (metrics == null) {
    return (
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-base">Speed Doctor</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-muted-foreground text-sm">
            No diagnostic data available.
          </p>
        </CardContent>
      </Card>
    );
  }

  const warnings = computeWarnings(metrics);
  const fileChanges = metrics.fileChangesSinceLastBuild;
  const totalChanges = metrics.fileChangesSinceLastBuildCount ?? 0;

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base">Speed Doctor</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        {warnings.length === 0 && (
          <p className="text-sm text-green-700 dark:text-green-400">
            No issues detected.
          </p>
        )}

        {warnings.map(w => (
          <div
            key={w.title}
            className={`rounded border p-3 ${severityStyles[w.severity]}`}>
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">{w.title}</span>
              <Badge
                variant={w.severity === 'error' ? 'destructive' : 'secondary'}>
                {w.severity}
              </Badge>
            </div>
            <p className="mt-1 text-xs">{w.description}</p>
          </div>
        ))}

        {totalChanges > 0 && (
          <details className="mt-2">
            <summary className="cursor-pointer text-sm font-medium">
              File changes since last build
              <Badge variant="secondary" className="ml-2">
                {totalChanges}
              </Badge>
            </summary>
            <ul className="mt-2 max-h-40 space-y-0.5 overflow-y-auto">
              {fileChanges.map((f, i) => (
                <li key={i} className="truncate font-mono text-xs">
                  {f}
                </li>
              ))}
              {fileChanges.length < totalChanges && (
                <li className="text-muted-foreground text-xs">
                  ... and {totalChanges - fileChanges.length} more
                </li>
              )}
            </ul>
          </details>
        )}
      </CardContent>
    </Card>
  );
}
