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
import java.util.Map;
import java.util.Objects;
import javax.annotation.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class ActionMetadata {

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
    return previousDigest.get(incrementalConfigFilePath);
  }

  @Nullable
  public String getCurrentIncrementalConfigDigest() {
    return currentDigest.get(incrementalConfigFilePath);
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
