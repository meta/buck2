/**
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

import {describe, it, expect} from 'vitest';
import {decompressZstd} from '../zstd';
import fs from 'fs';
import path from 'path';

describe('decompressZstd', () => {
  it('decompresses a small zstd-compressed payload', async () => {
    // Create a known payload: compress "hello world" with the zstd CLI
    // This test uses a fixture file if available, otherwise skips
    const fixturePath = path.join(__dirname, 'fixtures', 'hello.zst');
    if (!fs.existsSync(fixturePath)) {
      console.log('Skipping: fixture file not found at', fixturePath);
      return;
    }

    const compressed = new Uint8Array(fs.readFileSync(fixturePath));
    const decompressed = await decompressZstd(compressed);
    const text = new TextDecoder().decode(decompressed);
    expect(text).toBe('hello world\n');
  });

  it('decompresses a sample event log fixture', async () => {
    const fixturePath = path.join(
      __dirname,
      'fixtures',
      'sample-event-log.zst',
    );
    if (!fs.existsSync(fixturePath)) {
      console.log('Skipping: fixture file not found at', fixturePath);
      return;
    }

    const compressed = new Uint8Array(fs.readFileSync(fixturePath));
    const decompressed = await decompressZstd(compressed);

    // Basic sanity checks
    expect(decompressed.length).toBeGreaterThan(0);
    expect(decompressed.length).toBeGreaterThan(compressed.length);
  });

  it('throws on invalid input', async () => {
    const garbage = new Uint8Array([1, 2, 3, 4, 5]);
    await expect(decompressZstd(garbage)).rejects.toThrow();
  });
});
