/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.dex;

import com.facebook.infer.annotation.Nullsafe;

/** Options to pass to {@code d8}. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public enum D8Options {
  /** Specify the {@code --debug} flag. Otherwise --release is specified */
  NO_OPTIMIZE,

  /** Force the dexer to emit jumbo string references */
  FORCE_JUMBO,

  /** Disable java 8 desugaring when running D8 dexing tool. */
  NO_DESUGAR,

  /** Compile an intermediate result intended for later merging */
  INTERMEDIATE,

  /** Don't fill up the primary dex beyond classes that need to be in the primary dex */
  MINIMIZE_PRIMARY_DEX,

  /** Fill up the primary dex as much as possible */
  MAXIMIZE_PRIMARY_DEX,

  /** Fail if D8 produces multiple DEX files when a single output is expected */
  FAIL_ON_MULTIPLE_DEX,

  /** Write each class to its own DEX file in the output directory */
  FILE_PER_CLASS,
  ;
}
