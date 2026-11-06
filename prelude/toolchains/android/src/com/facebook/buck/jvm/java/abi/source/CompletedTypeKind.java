/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java.abi.source;

import com.facebook.infer.annotation.Nullsafe;

@Nullsafe(Nullsafe.Mode.LOCAL)
public enum CompletedTypeKind {
  COMPLETED_TYPE,
  PARTIALLY_COMPLETED_TYPE,
  ERROR_TYPE,
  CRASH;

  public CompletedTypeKind merge(CompletedTypeKind other) {
    if (this.compareTo(other) >= 0) {
      return this;
    }

    return other;
  }
}
