/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {decompress, Decompress} from 'fzstd';

/**
 * Decompress a zstd-compressed Uint8Array (all at once).
 * Uses fzstd (pure JS) which handles all frame types reliably.
 */
export async function decompressZstd(
  compressed: Uint8Array,
): Promise<Uint8Array> {
  return decompress(compressed);
}

/**
 * True streaming decompression using fzstd's Decompress class.
 * Feed compressed data in chunks, yield decompressed chunks as they're produced.
 * Never materializes the full decompressed output.
 *
 * @param compressed The full compressed input
 * @param inputChunkSize How many compressed bytes to feed per iteration (default 256KB)
 */
export async function* decompressZstdStreaming(
  compressed: Uint8Array,
  inputChunkSize = 256 * 1024,
): AsyncGenerator<Uint8Array> {
  let pending: Uint8Array[] = [];

  const decompressor = new Decompress(chunk => {
    pending.push(chunk);
  });

  for (let offset = 0; offset < compressed.length; offset += inputChunkSize) {
    const end = Math.min(offset + inputChunkSize, compressed.length);
    const isFinal = end >= compressed.length;
    decompressor.push(compressed.subarray(offset, end), isFinal);

    // Move pending chunks to a local, clear the closure-captured array
    const toYield = pending;
    pending = [];

    // Yield chunks one at a time, nulling each entry so it's GC-eligible
    // after the consumer processes it (otherwise the array holds all of them
    // alive until the loop completes)
    for (let i = 0; i < toYield.length; i++) {
      const chunk = toYield[i];
      toYield[i] = null as unknown as Uint8Array;
      yield chunk;
    }

    // Yield to event loop periodically so GC can run
    if (offset % (inputChunkSize * 4) === 0) {
      await new Promise(r => setTimeout(r, 0));
    }
  }
}

/**
 * Synchronous decompress using fzstd (pure JS, always available).
 * Used for on-demand chunk decompression on the main thread.
 */
export function decompressZstdSync(data: Uint8Array): Uint8Array {
  return decompress(data);
}
