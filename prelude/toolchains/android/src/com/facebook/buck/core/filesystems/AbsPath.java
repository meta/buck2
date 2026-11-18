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
import java.io.File;
import java.io.IOException;
import java.net.URI;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.Comparator;
import org.jspecify.annotations.Nullable;

/** Absolute path. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface AbsPath extends PathWrapper {
  /**
   * Cosnstruct using {@link java.nio.file.Path} object.
   *
   * @throws RuntimeException the path is not absolute.
   */
  static AbsPath of(Path path) {
    if (path instanceof AbsPath) {
      return (AbsPath) path;
    } else {
      return new AbsPathImpl(path);
    }
  }

  static AbsPath get(String path) {
    return AbsPath.of(Paths.get(path));
  }

  /** Behaves exactly like {@link Path#normalize()}. */
  default AbsPath normalize() {
    return of(getPath().normalize());
  }

  default AbsPath toRealPath(LinkOption... options) throws IOException {
    return AbsPath.of(getPath().toRealPath(options));
  }

  default AbsPath resolve(RelPath other) {
    return resolve(other.getPath());
  }

  default boolean startsWith(AbsPath path) {
    return startsWith(path.getPath());
  }

  /**
   * Remove prefix from this path, return {@code null} if this path does not start with given
   * prefix.
   */
  default @Nullable RelPath removePrefixIfStartsWith(AbsPath prefix) {
    if (!this.startsWith(prefix)) {
      return null;
    }
    return prefix.relativize(this);
  }

  /** Remove prefix from this path, throw if this path does not start with given prefix. */
  default RelPath removePrefix(AbsPath prefix) {
    RelPath relPath = removePrefixIfStartsWith(prefix);
    if (relPath == null) {
      throw new IllegalArgumentException(
          String.format("path %s does not start with %s", this, prefix));
    }
    return relPath;
  }

  default AbsPath resolve(Path path) {
    return AbsPath.of(getPath().resolve(path));
  }

  default AbsPath resolve(String path) {
    return AbsPath.of(getPath().resolve(path));
  }

  default @Nullable AbsPath getParent() {
    Path parent = getPath().getParent();
    return parent != null ? AbsPath.of(parent) : null;
  }

  default RelPath relativize(Path other) {
    return RelPath.of(this.getPath().relativize(other));
  }

  default RelPath relativize(AbsPath other) {
    return relativize(other.getPath());
  }

  /**
   * Get the filesystem root of the current path. Note unlike {@link Path#getRoot()} this function
   * never returns {@code null} because absolute paths always have root.
   */
  default AbsPath getRoot() {
    Path root = getPath().getRoot();
    if (root == null) {
      throw new IllegalStateException("abs path must have a root: " + this);
    }
    return AbsPath.of(root);
  }

  default File toFile() {
    return getPath().toFile();
  }

  /** We cannot implement {@link java.lang.Comparable} directly. */
  static Comparator<AbsPath> comparator() {
    return Comparator.comparing(AbsPath::getPath);
  }

  default URI toUri() {
    return getPath().toUri();
  }
}
