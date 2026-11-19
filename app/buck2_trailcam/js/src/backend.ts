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

import {createContext, createElement, useContext, type ReactNode} from 'react';

/** Text artifacts buck2 records next to the event log. */
export type LogTextKind = 'simpleconsole' | 'whatran' | 'expanded_command';

/** One earlier invocation, as listed in the build history sidebar. */
export interface BuildHistoryRow {
  uuid: string;
  command: string;
  hostname: string;
  isolation_dir: string;
  repository: string;
  username: string;
  outcome: string;
  is_success: number;
  duration_ms: number;
  /** Unix seconds. */
  wrapper_start_time: number;
  target_patterns: string;
  daemon_uuid: string;
  /** Path-like string ending in the CI job id, or empty for local builds. */
  sandcastle_job_info: string;
  /** Source-control revision the daemon was branched from (rebase base). */
  branched_from_revision: string;
  /**
   * Stateful max of jemalloc `stats.active` over the invocation, in bytes. A
   * reasonable proxy for the build's retained memory.
   */
  max_malloc_bytes_active: number;
}

export interface BuildHistoryQuery {
  username: string;
  hostname?: string | null;
  startTimeMs: number;
  endTimeMs: number;
  limit: number;
}

export interface FileChangeEntry {
  eventType: string;
  fileType: string;
  path: string;
}

export interface DaemonFileChangesResponse {
  /**
   * Every distinct file change since the daemon's last fresh instance, up to
   * and including this build.
   */
  changes: FileChangeEntry[];
  /** Number of builds the changes were accumulated over. */
  buildCount: number;
  daemonUuid: string;
  branchedFromRevision: string;
  buildUuids: string[];
}

/**
 * Everything the viewer needs from the server it is being served by.
 *
 * `fetchEventLog` is the only requirement. The rest are capabilities a host
 * may not have (a local viewer has no build history, for instance); the
 * components that depend on them render a reduced view when they are absent.
 */
export interface TrailcamBackend {
  /**
   * Fetch an event log as buck2 wrote it: zstd-compressed, length-delimited
   * `buck.data.CommandProgress` protobuf. `ref` is whatever the host handed
   * out as the log's identifier (a Manifold path, a local file, an object
   * key). Resolves to an OK response whose body is consumed as a stream;
   * `Content-Length`, when present, drives the progress display.
   */
  fetchEventLog(ref: string): Promise<Response>;

  /**
   * The console replay, what-ran listing, or expanded command line recorded
   * for an invocation. Null when the host has no such artifact.
   */
  fetchLogText?(uuid: string, kind: LogTextKind): Promise<string | null>;

  /** A URL a browser can download an artifact ref from (event log, RE log, trace). */
  artifactUrl?(ref: string): string;

  /** Earlier builds by the same user, for the history sidebar. */
  queryBuildHistory?(query: BuildHistoryQuery): Promise<BuildHistoryRow[]>;

  /**
   * File changes accumulated across the daemon session that ran `uuid`.
   * `buildStartTimeMs` lets the host centre its lookup on the build.
   */
  queryDaemonFileChanges?(
    uuid: string,
    buildStartTimeMs: number | null,
  ): Promise<DaemonFileChangesResponse>;
}

const BackendContext = createContext<TrailcamBackend | null>(null);

/**
 * Supplies the backend to every viewer component below it. Pass a stable
 * object: the data loaders re-run when the backend identity changes.
 */
export function BackendProvider({
  backend,
  children,
}: {
  backend: TrailcamBackend;
  children: ReactNode;
}) {
  return createElement(BackendContext.Provider, {value: backend}, children);
}

export function useBackend(): TrailcamBackend {
  const backend = useContext(BackendContext);
  if (backend == null) {
    throw new Error(
      'Trailcam components must be rendered inside <BackendProvider>',
    );
  }
  return backend;
}
