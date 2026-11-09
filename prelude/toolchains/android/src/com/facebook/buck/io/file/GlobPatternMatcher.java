/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.io.file;

import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.FileSystems;
import java.nio.file.Path;
import java.util.Objects;
import org.jspecify.annotations.Nullable;

/** Matcher that matches paths described by the glob pattern. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class GlobPatternMatcher implements PathMatcher {

  private final String globPattern;
  private final java.nio.file.PathMatcher globPatternMatcher;

  private GlobPatternMatcher(String gloPattern, java.nio.file.PathMatcher globPatternMatcher) {
    this.globPattern = gloPattern;
    this.globPatternMatcher = globPatternMatcher;
  }

  @Override
  public boolean equals(@Nullable Object other) {
    if (this == other) {
      return true;
    }
    if (!(other instanceof GlobPatternMatcher)) {
      return false;
    }
    GlobPatternMatcher that = (GlobPatternMatcher) other;
    // We don't compare globPatternMatcher here, since
    // sun.nio.fs.UnixFileSystem.getPathMatcher()
    // returns an anonymous class which doesn't implement equals().
    return Objects.equals(globPattern, that.globPattern);
  }

  @Override
  public int hashCode() {
    return Objects.hash(globPattern);
  }

  @Override
  public String toString() {
    return String.format("%s globPattern=%s", super.toString(), globPattern);
  }

  @Override
  public boolean matches(RelPath path) {
    return matches(path.getPath());
  }

  public boolean matches(Path path) {
    return globPatternMatcher.matches(path);
  }

  /**
   * @return The matcher for paths that start with {@code basePath}.
   */
  public static GlobPatternMatcher of(String globPattern) {
    return new GlobPatternMatcher(
        globPattern, FileSystems.getDefault().getPathMatcher("glob:" + globPattern));
  }
}
