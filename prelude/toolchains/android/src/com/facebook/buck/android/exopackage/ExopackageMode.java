/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.android.exopackage;

import com.facebook.infer.annotation.Nullsafe;
import java.util.EnumSet;

@Nullsafe(Nullsafe.Mode.LOCAL)
public enum ExopackageMode {
  SECONDARY_DEX(1),
  NATIVE_LIBRARY(2),
  RESOURCES(4),
  MODULES(8),
  ARCH64(16),
  ;

  private final int code;

  ExopackageMode(int code) {
    this.code = code;
  }

  public static boolean enabledForSecondaryDexes(EnumSet<ExopackageMode> modes) {
    return modes.contains(SECONDARY_DEX);
  }

  public static boolean enabledForNativeLibraries(EnumSet<ExopackageMode> modes) {
    return modes.contains(NATIVE_LIBRARY);
  }

  public static boolean enabledForResources(EnumSet<ExopackageMode> modes) {
    return modes.contains(RESOURCES);
  }

  public static boolean enabledForModules(EnumSet<ExopackageMode> modes) {
    return modes.contains(MODULES);
  }

  public static boolean enabledForArch64(EnumSet<ExopackageMode> modes) {
    return modes.contains(ARCH64);
  }

  public static int toBitmask(EnumSet<ExopackageMode> modes) {
    int bitmask = 0;
    for (ExopackageMode mode : modes) {
      bitmask |= mode.code;
    }
    return bitmask;
  }
}
