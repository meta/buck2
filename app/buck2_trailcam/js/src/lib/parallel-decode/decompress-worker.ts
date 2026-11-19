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
 * Decompression worker.
 *
 * Receives a ReadableStream of zstd-compressed bytes (transferred from
 * main, so the network response body streams directly into this worker),
 * drives fzstd's Decompress incrementally, and forwards each output
 * chunk to the orchestrator. This means decompression runs concurrently
 * with the network download — no need to buffer the full response first.
 */

import {Decompress} from 'fzstd';
import {PhaseTimer, SpanRecorder, nowMs} from '../phase-timer';
import type {DecompressRequest, DecompressResponse} from './types';

const post = (msg: DecompressResponse, transfer?: Transferable[]) => {
  if (transfer && transfer.length)
    (self as unknown as Worker).postMessage(msg, transfer);
  else (self as unknown as Worker).postMessage(msg);
};

// Worker's epoch (used by main to align this worker's spans with the
// others) is established by the `ping` handshake before any other work;
// the value lives on the main side in the lane's `epochMs`/`offsetMs`,
// not here.
const timer = new PhaseTimer();
const spans = new SpanRecorder();

self.onmessage = async (e: MessageEvent<DecompressRequest>) => {
  if (e.data.type === 'ping') {
    // Clock-sync handshake — record a tiny `handshake` span so main's
    // timeline has a visible sync point that lines up across all workers.
    const tEpoch = nowMs();
    spans.spans.push({phase: 'handshake', startMs: tEpoch, endMs: nowMs()});
    post({type: 'pong', epochMs: tEpoch, spans: spans.drain()});
    return;
  }
  if (e.data.type !== 'decompress') return;

  let decompressedBytes = 0;
  let receivedBytes = 0;

  try {
    // Set up the decompressor first so its `ondata` callback can post each
    // output chunk as it's produced.
    const decompressor = new Decompress(chunk => {
      decompressedBytes += chunk.length;
      // Copy into a fresh ArrayBuffer we can transfer. fzstd's internal
      // output buffer may be reused, so we can't transfer it directly.
      const buf = new ArrayBuffer(chunk.byteLength);
      new Uint8Array(buf).set(chunk);
      post({type: 'chunk', data: buf}, [buf]);
    });

    // Read the network stream; push each compressed chunk into fzstd as
    // it arrives. The first chunks of decompressed output start flowing
    // before the download is even half done.
    //
    // `network_read` spans cover each await reader.read() — they show
    // the worker waiting for more bytes from the network, and the *last*
    // network_read ends at the moment `done` was signaled (i.e., the
    // exact instant the response body finished arriving).
    const reader = e.data.stream.getReader();
    while (true) {
      const readSp = spans.begin('network_read');
      const {done, value} = await reader.read();
      readSp.end();
      if (done) {
        // Signal end-of-input with an empty buffer so fzstd flushes any
        // trailing block.
        const t = nowMs();
        const sp = spans.begin('decompress_chunk');
        decompressor.push(new Uint8Array(0), true);
        sp.end();
        timer.add('decompress', nowMs() - t);
        break;
      }
      receivedBytes += value.length;
      const t = nowMs();
      const sp = spans.begin('decompress_chunk');
      decompressor.push(value, false);
      sp.end();
      timer.add('decompress', nowMs() - t);
    }

    post({
      type: 'done',
      phaseTimings: timer.report(),
      decompressedBytes,
      receivedBytes,
      spans: spans.drain(),
    });
  } catch (err) {
    post({
      type: 'error',
      message: err instanceof Error ? err.message : 'decompress failed',
    });
  }
};
