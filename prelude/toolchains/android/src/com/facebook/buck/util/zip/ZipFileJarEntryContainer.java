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
import java.io.File;
import java.io.IOException;
import java.nio.file.Path;
import java.util.jar.JarFile;
import java.util.jar.Manifest;
import java.util.stream.Stream;
import java.util.zip.ZipEntry;
import javax.annotation.Nullable;

/** Provides all entries of a given zip or jar file, so they can be added to another jar. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class ZipFileJarEntryContainer implements JarEntryContainer {

  private final Path jarFilePath;
  private final boolean readOnly;
  @Nullable private JarFile jar;

  public ZipFileJarEntryContainer(Path jarFilePath) {
    this(jarFilePath, false);
  }

  public ZipFileJarEntryContainer(Path jarFilePath, boolean readOnly) {
    this.jarFilePath = jarFilePath;
    this.readOnly = readOnly;
  }

  @Nullable
  @Override
  public Manifest getManifest() throws IOException {
    return getJarFile().getManifest();
  }

  @Override
  public Stream<JarEntrySupplier> stream() throws IOException {
    return getJarFile().stream()
        .map(
            entry ->
                new JarEntrySupplier(
                    makeCustomEntry(entry), readOnly, () -> getJarFile().getInputStream(entry)));
  }

  @Override
  public void close() throws IOException {
    if (jar != null) {
      jar.close();
      jar = null;
    }
  }

  private JarFile getJarFile() throws IOException {
    if (jar == null) {
      try {
        File jarFile = jarFilePath.toFile();
        jar = new JarFile(jarFile);
      } catch (IOException e) {
        throw new IOException("Failed to process ZipFile " + jarFilePath, e);
      }
    }

    return jar;
  }

  private static CustomZipEntry makeCustomEntry(ZipEntry entry) {
    CustomZipEntry wrappedEntry = new CustomZipEntry(entry);

    // For deflated entries, the act of re-"putting" this entry means we're re-compressing
    // the data that we've just uncompressed.  Due to various environmental issues (e.g. a
    // newer version of zlib, changed compression settings), we may end up with a different
    // compressed size.  This causes an issue in java's `java.util.zip.ZipOutputStream`
    // implementation, as it only updates the compressed size field if one of `crc`,
    // `compressedSize`, or `size` is -1.  When we copy the entry as-is, none of these are
    // -1, and we may end up with an incorrect compressed size, in which case, we'll get an
    // exception.  So, for deflated entries, reset the compressed size to -1 (as the
    // ZipEntry(String) would).
    // See https://github.com/spearce/buck/commit/8338c1c3d4a546f577eed0c9941d9f1c2ba0a1b7.
    if (wrappedEntry.getMethod() == ZipEntry.DEFLATED) {
      wrappedEntry.setCompressedSize(-1);
    }

    return wrappedEntry;
  }
}
