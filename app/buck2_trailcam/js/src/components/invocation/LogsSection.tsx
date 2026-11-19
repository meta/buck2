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

import {Badge} from '../../ui';
import {useBackend} from '../../backend';

function ArtifactLink({
  artifactRef,
  unavailable,
}: {
  artifactRef: string | null;
  /** True when the host knows this artifact was never produced. */
  unavailable: boolean;
}) {
  const {artifactUrl} = useBackend();
  if (artifactRef) {
    return artifactUrl ? (
      <a
        href={artifactUrl(artifactRef)}
        target="_blank"
        rel="noopener noreferrer"
        className="text-xs text-blue-600 hover:underline dark:text-blue-400">
        Download
      </a>
    ) : (
      <Badge variant="secondary" className="text-[10px]">
        Available
      </Badge>
    );
  }
  return unavailable ? (
    <Badge variant="secondary" className="text-[10px]">
      Unavailable
    </Badge>
  ) : (
    <Badge variant="destructive" className="text-[10px]">
      Missing
    </Badge>
  );
}

/**
 * Bare event-log / RE-log download links. Designed to render inside the
 * DebugInfoPanel — no surrounding Card.
 */
export default function LogsSection({
  hasEventLog,
  eventLogPath,
  reLogPath,
  remoteExecutionId,
}: {
  hasEventLog: boolean | null;
  eventLogPath: string | null;
  reLogPath: string | null;
  remoteExecutionId: string | null;
}) {
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between">
        <span className="text-muted-foreground text-xs">Event Log</span>
        <ArtifactLink
          artifactRef={eventLogPath}
          unavailable={hasEventLog === false}
        />
      </div>
      <div className="flex items-center justify-between">
        <span className="text-muted-foreground text-xs">RE Log</span>
        <ArtifactLink
          artifactRef={reLogPath}
          unavailable={remoteExecutionId == null}
        />
      </div>
    </div>
  );
}
