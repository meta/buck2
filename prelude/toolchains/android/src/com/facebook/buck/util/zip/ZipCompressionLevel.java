/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.zip;

import com.facebook.infer.annotation.Nullsafe;

@Nullsafe(Nullsafe.Mode.LOCAL)
public enum ZipCompressionLevel {
  NONE(0),
  MIN(1),
  DEFAULT(6),
  MAX(9),
  ;

  private final int value;

  ZipCompressionLevel(int value) {
    this.value = value;
  }

  public int getValue() {
    return value;
  }
}
