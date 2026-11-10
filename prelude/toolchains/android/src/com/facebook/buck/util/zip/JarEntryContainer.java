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
import com.google.common.base.Preconditions;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.jar.Manifest;
import java.util.stream.Stream;
import org.jspecify.annotations.Nullable;

/**
 * Represents a container for entries to be added to a jar file by @{link JarBuilder}. Examples
 * include a directory tree or another jar file.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface JarEntryContainer extends AutoCloseable {

  /**
   * @param source Path to a jar file
   * @return Container which all the entries are not copyable
   * @see JarEntrySupplier#isReadOnly()
   */
  static JarEntryContainer readOnlyJar(Path source) {
    // Assume a zip or jar file.
    if (!Files.isRegularFile(source)) {
      throw new IllegalStateException("Must be a jar file: " + source);
    }
    return new ZipFileJarEntryContainer(source, true);
  }

  static JarEntryContainer of(Path source) {
    Preconditions.checkArgument(source.isAbsolute());

    if (Files.isDirectory(source)) {
      return new DirectoryJarEntryContainer(source);
    } else if (Files.isRegularFile(source)) {
      // Assume a zip or jar file.
      return new ZipFileJarEntryContainer(source);
    } else {
      throw new IllegalStateException("Must be a file or directory: " + source);
    }
  }

  @Nullable Manifest getManifest() throws IOException;

  Stream<JarEntrySupplier> stream() throws IOException;

  @Override
  void close() throws IOException;
}
