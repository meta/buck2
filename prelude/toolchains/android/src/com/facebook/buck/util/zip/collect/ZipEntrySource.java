/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip.collect;

import com.facebook.infer.annotation.Nullsafe;
import java.nio.file.Path;

/** Represents source for an entry in a zip file. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public interface ZipEntrySource {

  /**
   * Location of a file where source is located. Data for the entry can be located in a part of a
   * file.
   */
  Path getSourceFilePath();

  /** The name of the destination entry. */
  String getEntryName();
}
