/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ActionMetadata {
  // Buck2 names a content-based output directory after the first 8 bytes of the content digest.
  private static final int CONTENT_HASH_LENGTH = 16;

  private final Map<Path, String> previousDigest;
  private final Map<Path, String> currentDigest;
  private final Path incrementalConfigFilePath;

  public ActionMetadata(
      Path incrementalConfigFilePath,
      Map<Path, String> previousDigest,
      Map<Path, String> currentDigest) {
    this.previousDigest = previousDigest;
    this.currentDigest = currentDigest;
    this.incrementalConfigFilePath = incrementalConfigFilePath;
  }

  public Map<Path, String> getPreviousDigest() {
    return previousDigest;
  }

  public Map<Path, String> getCurrentDigest() {
    return currentDigest;
  }

  @Nullable
  public String getPreviousIncrementalConfigDigest() {
    @Nullable String digest = previousDigest.get(incrementalConfigFilePath);
    if (digest != null) {
      return digest;
    }
    // A content-based path changes with the content, so a changed config sits at a new path: find
    // the previous one by its path without the content hash.
    @Nullable String identity =
        withoutContentHash(incrementalConfigFilePath, getCurrentIncrementalConfigDigest());
    if (identity == null) {
      return null;
    }
    for (Map.Entry<Path, String> entry : previousDigest.entrySet()) {
      if (identity.equals(withoutContentHash(entry.getKey(), entry.getValue()))) {
        return entry.getValue();
      }
    }
    return null;
  }

  @Nullable
  public String getCurrentIncrementalConfigDigest() {
    return currentDigest.get(incrementalConfigFilePath);
  }

  /**
   * {@code path} without the segment that a content-based path takes from the start of its
   * content's {@code digest}, or null when the path has no such segment.
   */
  private static @Nullable String withoutContentHash(Path path, @Nullable String digest) {
    if (digest == null || digest.length() < CONTENT_HASH_LENGTH) {
      return null;
    }
    String contentHash = digest.substring(0, CONTENT_HASH_LENGTH);
    List<String> segments = new ArrayList<>();
    boolean found = false;
    for (Path segment : path) {
      if (!found && segment.toString().equals(contentHash)) {
        found = true;
      } else {
        segments.add(segment.toString());
      }
    }
    return found ? String.join("/", segments) : null;
  }

  @Override
  public boolean equals(@Nullable Object o) {
    if (this == o) return true;
    if (o == null || getClass() != o.getClass()) return false;
    ActionMetadata that = (ActionMetadata) o;
    return Objects.equals(previousDigest, that.previousDigest)
        && Objects.equals(currentDigest, that.currentDigest)
        && Objects.equals(incrementalConfigFilePath, that.incrementalConfigFilePath);
  }

  @Override
  public int hashCode() {
    return Objects.hash(previousDigest, currentDigest, incrementalConfigFilePath);
  }
}
