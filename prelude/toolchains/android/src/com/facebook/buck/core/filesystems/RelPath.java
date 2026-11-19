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
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Comparator;
import org.jspecify.annotations.Nullable;

/** Relative path. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface RelPath extends PathWrapper {

  /**
   * Construct using {@link java.nio.file.Path} object.
   *
   * @throws RuntimeException the path is not relative.
   */
  static RelPath of(Path path) {
    if (path instanceof RelPath) {
      return (RelPath) path;
    } else {
      return new RelPathImpl(path);
    }
  }

  /**
   * Construct a path.
   *
   * @throws RuntimeException if the path is absolute.
   */
  static RelPath get(String first, String... more) {
    return of(Paths.get(first, more));
  }

  /** Behaves exactly like {@link Path#normalize()}. */
  default RelPath normalize() {
    return of(getPath().normalize());
  }

  default @Nullable RelPath getParent() {
    Path parent = getPath().getParent();
    return parent != null ? RelPath.of(parent) : null;
  }

  default Path resolve(String other) {
    return getPath().resolve(other);
  }

  default Path resolve(Path other) {
    return getPath().resolve(other);
  }

  default RelPath resolveRel(String other) {
    return RelPath.of(resolve(other));
  }

  default RelPath resolve(RelPath other) {
    return RelPath.of(getPath().resolve(other.getPath()));
  }

  default RelPath subpath(int beginIndex, int endIndex) {
    return RelPath.of(getPath().subpath(beginIndex, endIndex));
  }

  default int getNameCount() {
    return getPath().getNameCount();
  }

  /** Convert path to an absolute path resolving it from current directory. */
  default AbsPath toAbsolutePath() {
    return AbsPath.of(getPath().toAbsolutePath());
  }

  default RelPath relativize(Path path) {
    return RelPath.of(getPath().relativize(path));
  }

  default RelPath relativize(RelPath path) {
    return relativize(path.getPath());
  }

  default boolean startsWith(RelPath path) {
    return getPath().startsWith(path.getPath());
  }

  /** We cannot implement {@link java.lang.Comparable} directly. */
  static Comparator<RelPath> comparator() {
    return Comparator.comparing(RelPath::getPath);
  }
}
