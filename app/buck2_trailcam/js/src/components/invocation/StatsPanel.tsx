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

import {Card, CardContent, CardHeader, CardTitle, Separator} from '../../ui';
import OutcomeBadge from './OutcomeBadge';
import {
  formatDuration,
  formatTimestamp,
  formatRelativeTime,
  formatCacheHitRate,
} from '../../lib/format';

function StatRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-baseline justify-between gap-2 py-1.5">
      <span className="text-muted-foreground shrink-0 text-sm">{label}</span>
      <span className="text-right text-sm font-medium">{children}</span>
    </div>
  );
}

export default function StatsPanel({
  durationMs,
  commandDurationMs,
  commandOutcome,
  startTime,
  creationTime,
  localActionsCount,
  remoteActionsCount,
  skippedActionsCount,
  cacheHitCount,
  cacheHitRate,
  firstBuildSinceRebase,
  errorMessages,
}: {
  durationMs: number | null;
  commandDurationMs: number | null;
  commandOutcome: string | null;
  startTime: number | null;
  creationTime: unknown;
  localActionsCount: number | null;
  remoteActionsCount: number | null;
  skippedActionsCount: number | null;
  cacheHitCount: number | null;
  cacheHitRate: number | null;
  firstBuildSinceRebase: boolean | null;
  errorMessages: ReadonlyArray<string | null> | null;
}) {
  const displayTime =
    startTime ?? (typeof creationTime === 'number' ? creationTime : null);

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base">Stats</CardTitle>
      </CardHeader>
      <CardContent className="space-y-1">
        <StatRow label="Duration">{formatDuration(commandDurationMs)}</StatRow>
        <StatRow label="Wall Time">{formatDuration(durationMs)}</StatRow>
        <StatRow label="Start Time">
          <span title={formatTimestamp(displayTime)}>
            {formatRelativeTime(displayTime)}
          </span>
        </StatRow>
        <StatRow label="Outcome">
          <OutcomeBadge outcome={commandOutcome} />
        </StatRow>

        <Separator className="my-2" />

        <StatRow label="Local Actions">{localActionsCount ?? '—'}</StatRow>
        <StatRow label="Remote Actions">{remoteActionsCount ?? '—'}</StatRow>
        <StatRow label="Skipped Actions">{skippedActionsCount ?? '—'}</StatRow>
        <StatRow label="Cache Hits">{cacheHitCount ?? '—'}</StatRow>
        <StatRow label="Cache Hit Rate">
          {formatCacheHitRate(cacheHitRate)}
        </StatRow>
        <StatRow label="Cold Build?">
          {firstBuildSinceRebase === true
            ? 'Yes'
            : firstBuildSinceRebase === false
              ? 'No'
              : 'Unknown'}
        </StatRow>
      </CardContent>
    </Card>
  );
}
