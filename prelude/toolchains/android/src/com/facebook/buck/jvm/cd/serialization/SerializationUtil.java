/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.serialization;

import com.facebook.infer.annotation.Nullsafe;

/** CD serialization utilities. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class SerializationUtil {

  private SerializationUtil() {}

  public static RuntimeException createNotSupportedException(Enum<?> value) {
    return new IllegalStateException(value.name() + " enum value is not supported!");
  }
}
