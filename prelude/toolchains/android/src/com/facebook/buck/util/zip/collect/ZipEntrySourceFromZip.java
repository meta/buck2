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

/** A source for a zip file entry that represents an entry for another zip file. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ZipEntrySourceFromZip implements ZipEntrySource {

  private final Path sourceFilePath;
  private final String entryName;
  private final int entryPosition;

  public ZipEntrySourceFromZip(Path sourceFilePath, String entryName, int entryPosition) {
    this.sourceFilePath = sourceFilePath;
    this.entryName = entryName;
    this.entryPosition = entryPosition;
  }

  /** Path to the source zip file. */
  @Override
  public Path getSourceFilePath() {
    return sourceFilePath;
  }

  /** The name of the entry */
  @Override
  public String getEntryName() {
    return entryName;
  }

  /** Position of the entry in the list of entries. */
  public int getEntryPosition() {
    return entryPosition;
  }

  @Override
  public int hashCode() {
    return Objects.hash(entryPosition, entryName, sourceFilePath);
  }

  @Override
  public boolean equals(@Nullable Object obj) {
    if (this == obj) {
      return true;
    }
    if (obj == null || getClass() != obj.getClass()) {
      return false;
    }
    final ZipEntrySourceFromZip other = (ZipEntrySourceFromZip) obj;
    return entryPosition == other.entryPosition
        && Objects.equals(entryName, other.entryName)
        && Objects.equals(sourceFilePath, other.sourceFilePath);
  }
}
