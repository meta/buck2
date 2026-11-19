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

import com.facebook.buck.cd.model.java.JarParameters;
import com.facebook.buck.jvm.java.RemoveClassesPatternsMatcher;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableSet;
import java.util.regex.Pattern;

/** {@link RemoveClassesPatternsMatcher} to protobuf serializer */
@Nullsafe(Nullsafe.Mode.LOCAL)
class RemoveClassesPatternsMatcherSerializer {

  private RemoveClassesPatternsMatcherSerializer() {}

  /**
   * Deserializes javacd model's {@link JarParameters.RemoveClassesPatternsMatcher} into {@link
   * RemoveClassesPatternsMatcher}.
   */
  public static RemoveClassesPatternsMatcher deserialize(
      JarParameters.RemoveClassesPatternsMatcher removeClassesPatternsMatcher) {
    ImmutableSet<Pattern> patterns =
        removeClassesPatternsMatcher.getPatternsList().stream()
            .map(Pattern::compile)
            .collect(ImmutableSet.toImmutableSet());
    return new RemoveClassesPatternsMatcher(patterns);
  }
}
