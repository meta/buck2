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

/** Reference-count field offsets, unchanged between the 112- and 120-byte DEX headers. */
@Nullsafe(Nullsafe.Mode.LOCAL)
final class DexHeader {
  static final int DEX_TYPE_IDS_SIZE_OFFSET = 64;
  static final int DEX_FIELD_IDS_SIZE_OFFSET = 80;
  static final int DEX_METHOD_IDS_SIZE_OFFSET = 88;

  private DexHeader() {}
}
