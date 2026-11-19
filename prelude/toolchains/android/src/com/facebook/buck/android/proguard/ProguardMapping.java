/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.proguard;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import java.util.Objects;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/** Parser for ProGuard-generated mapping files. Currently only handles class mapping. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class ProguardMapping {

  /** Utility class: do not instantiate. */
  private ProguardMapping() {}

  private static final Pattern CLASS_LINE_PATTERN = Pattern.compile("([-\\w.$]+) -> ([-\\w.$]+):");

  public static ImmutableMap<String, String> readClassMapping(Iterable<String> lines) {
    ImmutableMap.Builder<String, String> classMappingBuilder = ImmutableMap.builder();

    for (String line : lines) {
      if (line.charAt(0) == ' ') {
        // This is a member mapping, which we don't handle yet.
        continue;
      }

      Matcher matcher = CLASS_LINE_PATTERN.matcher(line);
      if (!matcher.matches()) {
        throw new IllegalArgumentException("Invalid line in proguard mapping: " + line);
      }

      classMappingBuilder.put(
          Objects.requireNonNull(matcher.group(1)), Objects.requireNonNull(matcher.group(2)));
    }

    return classMappingBuilder.build();
  }
}
