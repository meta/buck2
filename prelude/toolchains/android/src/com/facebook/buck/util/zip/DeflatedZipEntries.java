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

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.zip.CRC32;
import java.util.zip.Deflater;
import java.util.zip.DeflaterOutputStream;
import java.util.zip.ZipEntry;

/**
 * Writes deflated zip entries with their CRC and sizes in the local header and no data descriptor
 * after the data.
 *
 * <p>A streaming deflate only knows those values after the data, so it puts them in a data
 * descriptor. A tool that rewrites the archive without data descriptors but keeps each local header
 * and its padding, as the APK signing service does, then moves every later entry, and stored native
 * libraries lose their page alignment.
 */
public final class DeflatedZipEntries {
  private static final int BUFFER_SIZE = 64 * 1024;

  private DeflatedZipEntries() {}

  /**
   * Deflates {@code data} in memory at the entry's compression level, then writes the entry. Sets
   * the entry's method, CRC and sizes.
   */
  public static void write(CustomZipOutputStream out, CustomZipEntry entry, InputStream data)
      throws IOException {
    write(out, entry, deflate(data, entry.getCompressionLevel()));
  }

  /** Deflates {@code data} in memory at {@code level}, as {@link #write} does. */
  public static Deflated deflate(InputStream data, int level) throws IOException {
    CRC32 crc = new CRC32();
    ByteArrayOutputStream compressed = new ByteArrayOutputStream();
    long size = 0;
    Deflater deflater = new Deflater(level, /* nowrap= */ true);
    try (DeflaterOutputStream deflating =
        new DeflaterOutputStream(compressed, deflater, BUFFER_SIZE)) {
      byte[] buffer = new byte[BUFFER_SIZE];
      for (int count = data.read(buffer); count != -1; count = data.read(buffer)) {
        crc.update(buffer, 0, count);
        deflating.write(buffer, 0, count);
        size += count;
      }
    } finally {
      deflater.end();
    }
    return new Deflated(crc.getValue(), size, compressed);
  }

  /** Writes an entry whose data {@link #deflate} has already deflated. */
  public static void write(CustomZipOutputStream out, CustomZipEntry entry, Deflated deflated)
      throws IOException {
    entry.setMethod(ZipEntry.DEFLATED);
    entry.setRawCopy(true);
    entry.setCrc(deflated.crc);
    entry.setSize(deflated.size);
    entry.setCompressedSize(deflated.compressed.size());
    out.putNextEntry(entry);
    deflated.compressed.writeTo(out);
    out.closeEntry();
  }

  /** An entry's data deflated in memory, with its CRC and uncompressed size. */
  public static final class Deflated {
    private final long crc;
    private final long size;
    private final ByteArrayOutputStream compressed;

    private Deflated(long crc, long size, ByteArrayOutputStream compressed) {
      this.crc = crc;
      this.size = size;
      this.compressed = compressed;
    }
  }
}
