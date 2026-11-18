/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.core.filesystems;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.FileSystem;
import java.nio.file.Path;
import org.jspecify.annotations.Nullable;

/** {@link java.nio.file.Path} wrapper object, either absolute or relative. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface PathWrapper {
  Path getPath();

  @Override
  boolean equals(@Nullable Object that);

  /**
   * This overload should not be used.
   *
   * <p>This code is incorrect: <code>
   * AbsPath p1 = ...
   * Path p2 = ...
   * p1.equals(p2);
   * </code> because {@code AbsPath} and {@link Path} are different "types" and should not be equal
   * according to {@link Object#equals(Object)} contract.
   *
   * <p>This overload helps to catch this error.
   */
  @Deprecated
  default boolean equals(Path that) {
    throw new AssertionError();
  }

  @Override
  int hashCode();

  @Override
  String toString();

  default boolean endsWith(String other) {
    return getPath().endsWith(other);
  }

  default boolean endsWith(Path path) {
    return getPath().endsWith(path);
  }

  default FileSystem getFileSystem() {
    return getPath().getFileSystem();
  }

  default boolean startsWith(Path other) {
    return getPath().startsWith(other);
  }

  default boolean startsWith(String other) {
    return getPath().startsWith(other);
  }

  default Path getFileName() {
    return getPath().getFileName();
  }

  default Path getName(int index) {
    return getPath().getName(index);
  }
}
