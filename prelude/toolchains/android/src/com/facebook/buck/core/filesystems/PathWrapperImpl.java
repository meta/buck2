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
import org.jspecify.annotations.Nullable;

/** Base implementation of two path wrappers. Not visible to users. */
@Nullsafe(Nullsafe.Mode.LOCAL)
abstract class PathWrapperImpl implements PathWrapper {
  protected final Path path;

  protected PathWrapperImpl(Path path) {
    this.path = path;
  }

  @Override
  public Path getPath() {
    return path;
  }

  @Override
  public String toString() {
    return path.toString();
  }

  @Override
  public boolean equals(@Nullable Object obj) {
    if (this == obj) {
      return true;
    }
    if (obj == null || this.getClass() != obj.getClass()) {
      return false;
    }
    return this.path.equals(((PathWrapperImpl) obj).path);
  }

  @Override
  public int hashCode() {
    return path.hashCode();
  }
}
