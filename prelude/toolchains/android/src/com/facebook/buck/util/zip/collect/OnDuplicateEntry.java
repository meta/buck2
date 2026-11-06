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

/** Action to take when a duplicate entry is encountered when building a zip file. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public enum OnDuplicateEntry {
  /** Fail fast. */
  FAIL,
  /** An entry is overwritten. Last entry wins. */
  OVERWRITE,
  /** All entries are stored in the archive including the duplicates. */
  APPEND,
}
