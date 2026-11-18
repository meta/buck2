/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.classes;

import com.facebook.infer.annotation.Nullsafe;

/** Utilities for common operations when working with {@link FileLike}s. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class FileLikes {

  /** {@link FileLike} name suffix that identifies it as a Java class file. */
  private static final String CLASS_NAME_SUFFIX = ".class";

  /** Utility class: do not instantiate. */
  private FileLikes() {}

  public static boolean isClassFile(FileLike fileLike) {
    return fileLike.getRelativePath().endsWith(CLASS_NAME_SUFFIX);
  }

  public static String getFileNameWithoutClassSuffix(FileLike fileLike) {
    String name = fileLike.getRelativePath();
    return name.substring(0, name.length() - CLASS_NAME_SUFFIX.length());
  }
}
