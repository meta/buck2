/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.nio;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/** Helper class that actually maintains the underlying ByteBuffer for LargeByteBuffer. */
@Nullsafe(Nullsafe.Mode.LOCAL)
abstract class MappableByteBufferHolder {
  private final long size;
  private final int maxBufferSize;

  private long windowStart = -1;
  private long windowEnd = -1;
  private ByteOrder byteOrder = ByteOrder.BIG_ENDIAN;
  private @Nullable ByteBuffer byteBuffer = null;

  MappableByteBufferHolder(long size, int maxBufferSize) throws IOException {
    this.size = size;
    this.maxBufferSize = maxBufferSize;

    // If the file is small enough to be mapped all at once, go ahead and do it.
    if (size <= maxBufferSize) {
      mapByteBuffer(0, (int) size);
    }
  }

  /** Implement this to actually build the new ByteBuffer as it moves. */
  protected abstract ByteBuffer map(long start, int length) throws IOException;

  public long getSize() {
    return size;
  }

  /** Sets the order on the current ByteBuffer and retains it for all future ones. */
  public void order(ByteOrder byteOrder) {
    this.byteOrder = byteOrder;
    if (byteBuffer != null) {
      byteBuffer.order(byteOrder);
    }
  }

  /** Gets the order used for all underlying ByteBuffers. */
  public ByteOrder order() {
    return this.byteOrder;
  }

  /** Releases the ByteBuffer memory if it is allocated. */
  public void releaseByteBufferMemory() {
    if (byteBuffer != null) {
      ByteBufferUnmapper.unmap(byteBuffer);
      byteBuffer = null;
    }
  }

  /** Recreates the ByteBuffer, mapping it to a new window of the data. */
  private void mapByteBuffer(long start, int length) throws IOException {
    // Release the old data to prevent memory leaks;
    releaseByteBufferMemory();

    // Update the window and rebuild the buffer objects.
    windowStart = start;
    windowEnd = start + length;
    byteBuffer = map(start, length);

    // Restore the byte order.
    if (byteOrder != null) {
      byteBuffer.order(byteOrder);
    }
  }

  /** Checks that the current ByteBuffer window contains the requested data. */
  public void updateWindow(long start, long length) throws IOException {
    if (length > maxBufferSize) {
      throw new IllegalArgumentException(
          String.format(
              "SlidingFileLargeByteBuffer working data exceeds maximum buffer size (%s bytes)",
              maxBufferSize));
    }

    long end = start + length;

    // If the window already contains everything we need, then don't update it.
    if (start >= windowStart && end <= windowEnd) {
      return;
    }

    if (start < 0 || end > size) {
      throw new IndexOutOfBoundsException(
          String.format(
              "Tried to create window from %s to %s (only %s bytes available)", start, end, size));
    }

    // If we're here, then the entire file can't be mapped into memory.
    // Create a new window, at maximum size, centered around the start point.
    long newStart = start;

    // Make sure the window is within the bounds of the underlying file.
    if (newStart < 0) {
      newStart = 0;
    } else if (newStart + maxBufferSize > size) {
      newStart = size - maxBufferSize;
    }

    mapByteBuffer(newStart, maxBufferSize);
  }

  /** Gets the relative index to use with the ByteBuffer for a given position. */
  public int getRelativeIndex(long index) {
    return (int) (index - windowStart);
  }

  public ByteBuffer getByteBuffer() {
    return Objects.requireNonNull(byteBuffer);
  }
}
