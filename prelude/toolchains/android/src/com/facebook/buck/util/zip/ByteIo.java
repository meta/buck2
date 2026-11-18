/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.io.OutputStream;

/** Zip implementation assume bytes to be in Intel (little-endian) byte order. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ByteIo {

  private ByteIo() {
    // utility class.
  }

  public static int readShort(byte[] b, int off) {
    return (b[off] & 0xff) | ((b[off + 1] & 0xff) << 8);
  }

  public static int readInt(byte[] b, int off) {
    return (int) readUnsignedInt(b, off);
  }

  public static long readUnsignedInt(byte[] b, int off) {
    return (readShort(b, off) | ((long) readShort(b, off + 2) << 16)) & 0xffffffffL;
  }

  public static long readLong(byte[] b, int off) {
    return readUnsignedInt(b, off) | (readUnsignedInt(b, off + 4) << 32);
  }

  public static long writeShort(OutputStream out, int value) throws IOException {
    out.write((value & 0xff));
    out.write(((value >>> 8) & 0xff));
    return 2;
  }

  protected static long writeInt(OutputStream out, long value) throws IOException {
    out.write((int) (value & 0xff));
    out.write((int) ((value >>> 8) & 0xff));
    out.write((int) ((value >>> 16) & 0xff));
    out.write((int) ((value >>> 24) & 0xff));
    return 4;
  }

  protected static long writeLong(OutputStream out, long value) throws IOException {
    out.write((int) (value & 0xff));
    out.write((int) ((value >>> 8) & 0xff));
    out.write((int) ((value >>> 16) & 0xff));
    out.write((int) ((value >>> 24) & 0xff));
    out.write((int) ((value >>> 32) & 0xff));
    out.write((int) ((value >>> 40) & 0xff));
    out.write((int) ((value >>> 48) & 0xff));
    out.write((int) ((value >>> 56) & 0xff));
    return 8;
  }
}
