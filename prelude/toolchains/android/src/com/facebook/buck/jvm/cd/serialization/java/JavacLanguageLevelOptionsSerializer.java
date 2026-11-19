/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.serialization.java;

import com.facebook.buck.cd.model.java.ResolvedJavacOptions;
import com.facebook.buck.jvm.java.JavacLanguageLevelOptions;
import com.facebook.infer.annotation.Nullsafe;

/** {@link JavacLanguageLevelOptions} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
class JavacLanguageLevelOptionsSerializer {

  private JavacLanguageLevelOptionsSerializer() {}

  /**
   * Deserializes javacd model's {@link ResolvedJavacOptions.JavacLanguageLevelOptions} into {@link
   * JavacLanguageLevelOptions}.
   */
  public static JavacLanguageLevelOptions deserialize(
      ResolvedJavacOptions.JavacLanguageLevelOptions javacLanguageLevelOptions) {
    return new JavacLanguageLevelOptions(
        javacLanguageLevelOptions.getSourceLevel(), javacLanguageLevelOptions.getTargetLevel());
  }
}
