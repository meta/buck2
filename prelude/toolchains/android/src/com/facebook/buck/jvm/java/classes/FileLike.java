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
import java.io.IOException;
import java.io.InputStream;
import java.nio.file.Path;

/**
 * Provides a file-like interface for objects which may be present within more specialized
 * containers (like zip files).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface FileLike {
  /** Returns the containing file for this file-like entry. */
  Path getContainer();

  /**
   * Returns the relative path for the entry. For example, if this were a zip file, this would be
   * the relative path of a particular item in the zip file.
   */
  String getRelativePath();

  /** Returns the size of the entry in bytes. */
  long getSize() throws IOException;

  /**
   * Opens a new input stream for the entry. This can be repeated to open the file-like object
   * multiple times.
   *
   * @return Newly opened input stream.
   * @throws java.io.IOException An error occurred opening the stream.
   */
  InputStream getInput() throws IOException;
}
