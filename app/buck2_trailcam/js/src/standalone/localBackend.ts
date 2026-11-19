/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import type {TrailcamBackend} from '../backend';

async function failOnError(res: Response, what: string): Promise<Response> {
  if (!res.ok) throw new Error(`Failed to fetch ${what}: HTTP ${res.status}`);
  return res;
}

/**
 * Backend for a server that hosts the bundle next to a single event log, such
 * as `buck2 log trailcam`. The routes it needs:
 *
 *   GET /api/invocation   -> InvocationInfo as JSON
 *   GET /api/event-log    -> the event log, zstd-compressed length-delimited
 *                            CommandProgress protobuf
 */
export const localBackend: TrailcamBackend = {
  fetchEventLog: () =>
    fetch('/api/event-log', {cache: 'no-store'}).then(res =>
      failOnError(res, 'event log'),
    ),
};
