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

import org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.fir.extensions.FirAnalysisHandlerExtension
import org.jetbrains.kotlin.fir.extensions.FirExtensionRegistrarAdapter

@OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)
@SuppressWarnings("PackageLocationMismatch")
class K2JvmAbiComponentRegistrar : CompilerPluginRegistrar() {
  override val pluginId: String = "com.facebook.kotlin.k2.jvm.abi.gen"

  override fun ExtensionStorage.registerExtensions(configuration: CompilerConfiguration) {
    val outputPath = configuration.get(K2JvmAbiConfigurationKeys.OUTPUT_PATH)

    if (outputPath != null) {
      // Register K2 FIR extension for ABI generation
      FirAnalysisHandlerExtension.registerExtension(K2JvmAbiFirAnalysisHandlerExtension(outputPath))
      FirExtensionRegistrarAdapter.registerExtension(AbiGenFirExtensionRegistrar())
    }
  }

  override val supportsK2: Boolean
    get() = true
}
