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
 * Facts about the machine and daemon that ran a build, as measured by the
 * host's own telemetry rather than read from the event log. Hosts without
 * such telemetry pass null and the dependent panels say so.
 */
export interface InvocationMetrics {
  peakUsedDiskSpaceBytes: number | null;
  totalDiskSpaceBytes: number | null;
  peakProcessMemoryBytes: number | null;
  systemTotalMemoryBytes: number | null;
  /**
   * Paths changed since the previous build on this daemon, in buck2's
   * `<event type>:<file type>:<path>` encoding. May be truncated; the count
   * is authoritative.
   */
  fileChangesSinceLastBuild: readonly string[];
  fileChangesSinceLastBuildCount: number | null;
  /** Other buck2 commands that ran at the same time on the same daemon. */
  concurrentCommandIds: readonly string[];
  isolationDir: string | null;
  repository: string | null;
}

/**
 * Remote execution transfer statistics. Decimal strings rather than numbers:
 * the 64-bit byte counters come out of GraphQL as strings, and hosts that
 * compute them pass them along unchanged.
 */
export interface RemoteExecutionStats {
  uploadSpeedMax: string | null;
  uploadSpeedAvg: string | null;
  downloadSpeedMax: string | null;
  downloadSpeedAvg: string | null;
  bytesUploaded: string | null;
  bytesDownloaded: string | null;
}

/**
 * What the host knows about an invocation before the event log is decoded.
 * A host backed by a build index fills most of this in; a host that only has
 * the log file leaves the rest null and the panels degrade accordingly.
 */
export interface InvocationInfo {
  uuid: string;
  /** The buck2 subcommand: `build`, `test`, ... */
  command: string | null;
  cliArgs: readonly string[];
  commandOutcome: string | null;
  username: string | null;
  hostname: string | null;
  client: string | null;
  durationMs: number | null;
  commandDurationMs: number | null;
  /** Unix seconds. */
  startTime: number | null;
  /** Unix seconds. */
  creationTime: number | null;
  localActionsCount: number | null;
  remoteActionsCount: number | null;
  skippedActionsCount: number | null;
  cacheHitCount: number | null;
  cacheHitRate: number | null;
  firstBuildSinceRebase: boolean | null;
  errorMessages: readonly string[];
  buck2Revision: string | null;
  remoteExecutionId: string | null;
  remoteExecution: RemoteExecutionStats | null;
  /** Backend ref of the event log; see `TrailcamBackend.fetchEventLog`. */
  eventLogRef: string | null;
  /** False when the host knows no event log was ever recorded. */
  hasEventLog: boolean | null;
  /** Backend ref of the remote execution log, if any. */
  reLogRef: string | null;
}
