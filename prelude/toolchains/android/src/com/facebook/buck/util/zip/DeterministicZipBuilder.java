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
import com.google.common.io.ByteStreams;
import java.io.Closeable;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.zip.CRC32;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class DeterministicZipBuilder implements Closeable {
  // TODO(cjhopman): Should this buffer the entries and then sort them by name? We may have to
  // buffer them on disk to keep memory use sensible.
  private final CustomZipOutputStream output;

  public DeterministicZipBuilder(Path path) throws IOException {
    this.output = ZipOutputStreams.newOutputStream(path);
  }

  public void addEntry(
      InputStream data,
      long dataLength,
      long crc,
      String name,
      int compressionLevel,
      boolean isDirectory)
      throws IOException {
    CustomZipEntry outputEntry = new CustomZipEntry(Paths.get(name), isDirectory);
    outputEntry.setCompressionLevel(compressionLevel);
    outputEntry.setCrc(crc);
    if (compressionLevel == 0) {
      outputEntry.setCompressedSize(dataLength);
    }
    outputEntry.setSize(dataLength);
    output.putNextEntry(outputEntry);
    ByteStreams.copy(data, output);
    output.closeEntry();
  }

  public void addEntry(byte[] data, String name, int compressionLevel) throws IOException {
    CustomZipEntry outputEntry = new CustomZipEntry(Paths.get(name));
    outputEntry.setCompressionLevel(compressionLevel);
    CRC32 crc = new CRC32();
    crc.update(data);
    outputEntry.setCrc(crc.getValue());
    if (compressionLevel == 0) {
      outputEntry.setCompressedSize(data.length);
    }
    outputEntry.setSize(data.length);
    output.putNextEntry(outputEntry);
    output.write(data);
    output.closeEntry();
  }

  /**
   * Adds a pre-compressed entry to the zip. The data is written directly without decompression or
   * re-compression. The caller must provide the original entry's CRC, uncompressed size, compressed
   * size, and compression method.
   */
  public void addRawEntry(
      InputStream compressedData,
      long compressedSize,
      long uncompressedSize,
      long crc,
      int method,
      String name)
      throws IOException {
    CustomZipEntry outputEntry = new CustomZipEntry(Paths.get(name));
    outputEntry.setMethod(method);
    outputEntry.setCrc(crc);
    outputEntry.setSize(uncompressedSize);
    outputEntry.setCompressedSize(compressedSize);
    outputEntry.setRawCopy(true);
    output.putNextEntry(outputEntry);
    ByteStreams.copy(compressedData, output);
    output.closeEntry();
  }

  @Override
  public void close() throws IOException {
    output.close();
  }
}
