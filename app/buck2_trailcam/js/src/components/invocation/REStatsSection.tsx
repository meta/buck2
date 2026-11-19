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
import {formatSpeed, formatBytes} from '../../lib/format';

function StatPair({
  label,
  maxValue,
  avgValue,
}: {
  label: string;
  maxValue: string;
  avgValue: string;
}) {
  return (
    <div className="py-1.5">
      <span className="text-muted-foreground text-sm">{label}</span>
      <div className="mt-1 flex gap-4">
        <div>
          <span className="text-muted-foreground text-xs">Max</span>
          <p className="text-sm font-medium">{maxValue}</p>
        </div>
        <div>
          <span className="text-muted-foreground text-xs">Avg</span>
          <p className="text-sm font-medium">{avgValue}</p>
        </div>
      </div>
    </div>
  );
}

export default function REStatsSection({
  uploadSpeedMax,
  uploadSpeedAvg,
  downloadSpeedMax,
  downloadSpeedAvg,
  bytesUploaded,
  bytesDownloaded,
}: {
  uploadSpeedMax: string | null;
  uploadSpeedAvg: string | null;
  downloadSpeedMax: string | null;
  downloadSpeedAvg: string | null;
  bytesUploaded: string | null;
  bytesDownloaded: string | null;
}) {
  const hasUpload = uploadSpeedMax !== '0' && uploadSpeedMax != null;
  const hasDownload = downloadSpeedMax !== '0' && downloadSpeedMax != null;

  if (!hasUpload && !hasDownload) {
    return null;
  }

  return (
    <Card>
      <CardHeader className="pb-2">
        <CardTitle className="text-base">Remote Execution Stats</CardTitle>
      </CardHeader>
      <CardContent>
        {hasUpload && (
          <StatPair
            label="Upload Speed"
            maxValue={formatSpeed(uploadSpeedMax)}
            avgValue={formatSpeed(uploadSpeedAvg)}
          />
        )}
        {hasDownload && (
          <StatPair
            label="Download Speed"
            maxValue={formatSpeed(downloadSpeedMax)}
            avgValue={formatSpeed(downloadSpeedAvg)}
          />
        )}
        <Separator className="my-2" />
        <div className="flex gap-4 py-1.5">
          <div>
            <span className="text-muted-foreground text-xs">Uploaded</span>
            <p className="text-sm font-medium">{formatBytes(bytesUploaded)}</p>
          </div>
          <div>
            <span className="text-muted-foreground text-xs">Downloaded</span>
            <p className="text-sm font-medium">
              {formatBytes(bytesDownloaded)}
            </p>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
