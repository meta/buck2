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
import java.nio.file.Path;

/** A contract for matching {@link Path}s. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface PathMatcher {

  /** Check if given path matches pattern. */
  boolean matches(RelPath path);
}
