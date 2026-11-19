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
 * LRU cache for decompressed event log chunks.
 * Bounded by total byte size, evicts least-recently-used chunks.
 */
export class ChunkLRUCache {
  private cache = new Map<number, Uint8Array>();
  private accessOrder: number[] = [];
  private currentSize = 0;
  private readonly maxSize: number;

  constructor(maxSizeBytes = 30 * 1024 * 1024) {
    this.maxSize = maxSizeBytes;
  }

  get(chunkIndex: number): Uint8Array | undefined {
    const data = this.cache.get(chunkIndex);
    if (data) {
      // Move to end of access order (most recently used)
      const idx = this.accessOrder.indexOf(chunkIndex);
      if (idx >= 0) this.accessOrder.splice(idx, 1);
      this.accessOrder.push(chunkIndex);
    }
    return data;
  }

  put(chunkIndex: number, data: Uint8Array): void {
    // If already cached, remove old entry first
    if (this.cache.has(chunkIndex)) {
      const old = this.cache.get(chunkIndex)!;
      this.currentSize -= old.length;
      this.cache.delete(chunkIndex);
      const idx = this.accessOrder.indexOf(chunkIndex);
      if (idx >= 0) this.accessOrder.splice(idx, 1);
    }

    // Evict LRU entries until we have room
    while (
      this.currentSize + data.length > this.maxSize &&
      this.accessOrder.length > 0
    ) {
      const evictIdx = this.accessOrder.shift()!;
      const evicted = this.cache.get(evictIdx);
      if (evicted) {
        this.currentSize -= evicted.length;
        this.cache.delete(evictIdx);
      }
    }

    this.cache.set(chunkIndex, data);
    this.accessOrder.push(chunkIndex);
    this.currentSize += data.length;
  }

  get size(): number {
    return this.currentSize;
  }

  get count(): number {
    return this.cache.size;
  }

  clear(): void {
    this.cache.clear();
    this.accessOrder = [];
    this.currentSize = 0;
  }
}
