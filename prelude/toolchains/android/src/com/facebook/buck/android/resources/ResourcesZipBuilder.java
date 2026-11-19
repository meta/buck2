/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.resources;

import com.facebook.buck.util.zip.DeterministicZipBuilder;
import com.facebook.infer.annotation.Nullsafe;
import java.io.ByteArrayInputStream;
import java.io.Closeable;
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Path;
import java.util.zip.CRC32;
import java.util.zip.Deflater;

/**
 * Some versions of Android require that any zip file that contains resources/assets contains an
 * AndroidManifest.xml (though it doesn't actually read it). This is just a zip builder that will
 * add an empty (*) manifest if none has been added when it is closed.
 *
 * <p>See
 * https://android.googlesource.com/platform/frameworks/base/+/lollipop-release/libs/androidfw/AssetManager.cpp#209
 *
 * <p>(*) Due to a bug in Android's zip handling, we actually create a 1-byte manifest... See
 * https://android.googlesource.com/platform/system/core/+/48953a1b8fdcf1d6fa1aeeb40c57821d33fc87d2
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ResourcesZipBuilder implements Closeable {
  public static final String ANDROID_MANIFEST_XML = "AndroidManifest.xml";
  private boolean needsManifest;
  private DeterministicZipBuilder builder;

  public ResourcesZipBuilder(Path path) throws IOException {
    this(path, true);
  }

  public ResourcesZipBuilder(Path path, boolean addManifestIfMissing) throws IOException {
    builder = new DeterministicZipBuilder(path);
    needsManifest = addManifestIfMissing;
  }

  public void addEntry(
      InputStream stream,
      long size,
      long crc,
      String name,
      int compressionLevel,
      boolean isDirectory)
      throws IOException {
    builder.addEntry(stream, size, crc, name, compressionLevel, isDirectory);
    if (name.equals(ANDROID_MANIFEST_XML)) {
      needsManifest = false;
    }
  }

  /**
   * Adds a pre-compressed entry to the zip without decompressing or re-compressing. The raw
   * compressed bytes are written directly to the output.
   */
  public void addRawEntry(
      InputStream compressedData,
      long compressedSize,
      long uncompressedSize,
      long crc,
      int method,
      String name)
      throws IOException {
    builder.addRawEntry(compressedData, compressedSize, uncompressedSize, crc, method, name);
    if (name.equals(ANDROID_MANIFEST_XML)) {
      needsManifest = false;
    }
  }

  @Override
  public void close() throws IOException {
    if (needsManifest) {
      byte[] data = new byte[1];
      CRC32 crc32 = new CRC32();
      crc32.update(data);
      addEntry(
          new ByteArrayInputStream(data),
          1,
          crc32.getValue(),
          ANDROID_MANIFEST_XML,
          Deflater.NO_COMPRESSION,
          false);
    }
    builder.close();
  }
}
