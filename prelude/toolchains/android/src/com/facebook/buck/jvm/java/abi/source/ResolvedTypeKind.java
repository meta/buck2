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
enum ResolvedTypeKind {
  /** Type would resolve successfully. */
  RESOLVED_TYPE,
  /** Type would resolve as an {@link javax.lang.model.type.ErrorType}. */
  ERROR_TYPE,
  /** The compiler would crash trying to resolve the type. */
  CRASH;

  public ResolvedTypeKind merge(ResolvedTypeKind other) {
    return (this.compareTo(other) >= 0) ? this : other;
  }
}
