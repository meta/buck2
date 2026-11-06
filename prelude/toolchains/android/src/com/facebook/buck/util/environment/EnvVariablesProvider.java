/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.environment;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import java.util.Map;
import java.util.Objects;

/** Provides access system environment variables of the current process. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class EnvVariablesProvider {

  @SuppressWarnings("PMD.BlacklistedSystemGetenv")
  public static ImmutableMap<String, String> getSystemEnv() {
    if (Platform.detect().getType() == PlatformType.WINDOWS) {
      return System.getenv().entrySet().stream()
          .collect(ImmutableMap.toImmutableMap(e -> e.getKey().toUpperCase(), Map.Entry::getValue));
    } else {
      return ImmutableMap.copyOf(System.getenv());
    }
  }

  public static String getRequiredEnvVar(String key) {
    return Objects.requireNonNull(
        getSystemEnv().get(key), String.format("%S must be set in environment", key));
  }
}
