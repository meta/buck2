/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook

import org.jetbrains.kotlin.config.CompilerConfigurationKey

@SuppressWarnings("PackageLocationMismatch")
object K2JvmAbiConfigurationKeys {
  val OUTPUT_PATH: CompilerConfigurationKey<String> =
      CompilerConfigurationKey.create("output path for K2 JVM ABI generation")

  val EARLY_TERMINATION: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("early termination flag for K2 JVM ABI generation")

  val ENABLE_MIXED_COMPILATION: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("enable mixed compilation for K2 JVM ABI generation")

  val DEBUG_OUTPUT: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("debug output flag for K2 JVM ABI generation")

  val ENABLE_COMPOSE_ABI_EMULATION: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("enable Compose ABI emulation for K2 JVM ABI generation")

  val AST_PACKAGE_NAMES: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("read source packages from the AST, not PSI stub trees")

  val ABI_VALIDATION_MODE: CompilerConfigurationKey<String> =
      CompilerConfigurationKey.create("ABI validation mode for K2 JVM ABI generation")

  val SHALLOW_CLASS_USAGE: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("shallow class usage for K2 JVM ABI dep files")

  val SKIP_NON_ABI_BODIES: CompilerConfigurationKey<Boolean> =
      CompilerConfigurationKey.create("skip resolving bodies the source-only ABI never reads")
}
