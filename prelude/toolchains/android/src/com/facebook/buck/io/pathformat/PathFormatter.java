/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.io.pathformat;

import com.facebook.buck.core.filesystems.PathWrapper;
import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;

/** Utilities to format paths with predictable path separators. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PathFormatter {
  /** Utility class: do not instantiate. */
  private PathFormatter() {}

  public static String pathWithUnixSeparators(String path) {
    return path.replace('\\', '/');
  }

  public static String pathWithUnixSeparators(Path path) {
    return pathWithUnixSeparators(path.toString());
  }

  public static String pathWithUnixSeparators(PathWrapper path) {
    return pathWithUnixSeparators(path.getPath());
  }

  public static String pathWithWindowsSeparators(Path path) {
    return path.toString().replace('/', '\\');
  }

  public static String pathWithUnixSeparatorsAndTrailingSlash(Path path) {
    return pathWithUnixSeparators(path) + "/";
  }
}
