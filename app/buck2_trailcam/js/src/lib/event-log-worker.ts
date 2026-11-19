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
 * Web Worker for event log decompression and decoding.
 * Runs zstd decompression + protobuf decode off the main thread.
 *
 * Events are streamed as batches. Each batch contains:
 * - summaries: lightweight EventSummary[] (structured clone, small)
 * - protoBuffer: ArrayBuffer of raw protobuf bytes (transferred, zero-copy)
 */

import {decompressZstd} from './zstd';
import {decodeEventLogWithSummaries} from './event-log-decoder';
import type {EventSummary} from './event-log-decoder';

export interface WorkerRequest {
  type: 'decode';
  compressed: ArrayBuffer;
}

export type WorkerResponse =
  | {type: 'progress'; message: string}
  | {type: 'batch'; summaries: EventSummary[]; protoBuffer: ArrayBuffer}
  | {
      type: 'done';
      rawSize: number;
      decompressedSize: number;
      totalEvents: number;
    }
  | {type: 'error'; message: string};

self.onmessage = async (e: MessageEvent<WorkerRequest>) => {
  if (e.data.type !== 'decode') return;

  try {
    const compressed = new Uint8Array(e.data.compressed);
    const rawSize = compressed.length;

    self.postMessage({type: 'progress', message: 'Decompressing...'});
    const decompressed = await decompressZstd(compressed);
    const decompressedSize = decompressed.length;

    self.postMessage({type: 'progress', message: 'Decoding events...'});

    let totalEvents = 0;
    for (const batch of decodeEventLogWithSummaries(decompressed, 10000)) {
      self.postMessage(
        {
          type: 'batch',
          summaries: batch.summaries,
          protoBuffer: batch.protoBuffer,
        } as WorkerResponse,
        {transfer: [batch.protoBuffer]},
      );
      totalEvents += batch.summaries.length;
    }

    self.postMessage({
      type: 'done',
      rawSize,
      decompressedSize,
      totalEvents,
    });
  } catch (err) {
    self.postMessage({
      type: 'error',
      message: err instanceof Error ? err.message : 'Worker decode failed',
    });
  }
};
