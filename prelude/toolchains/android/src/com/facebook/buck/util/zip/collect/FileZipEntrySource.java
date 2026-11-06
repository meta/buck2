/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip.collect;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/** A file source for an entry in a zip file. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class FileZipEntrySource implements ZipEntrySource {

  private final Path sourceFilePath;
  private final String entryName;

  public FileZipEntrySource(Path sourceFilePath, String entryName) {
    this.sourceFilePath = sourceFilePath;
    this.entryName = entryName;
  }

  @Override
  public Path getSourceFilePath() {
    return sourceFilePath;
  }

  @Override
  public String getEntryName() {
    return entryName;
  }

  @Override
  public boolean equals(@Nullable Object o) {
    if (this == o) {
      return true;
    }
    if (o == null || getClass() != o.getClass()) {
      return false;
    }
    FileZipEntrySource that = (FileZipEntrySource) o;
    return Objects.equals(sourceFilePath, that.sourceFilePath)
        && Objects.equals(entryName, that.entryName);
  }

  @Override
  public int hashCode() {
    return Objects.hash(sourceFilePath, entryName);
  }
}
