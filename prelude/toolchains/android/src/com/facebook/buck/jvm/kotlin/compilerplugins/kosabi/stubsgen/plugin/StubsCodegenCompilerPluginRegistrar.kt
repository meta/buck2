/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.plugin.stubsgen_k2

import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import java.io.File
import org.jetbrains.kotlin.cli.jvm.config.jvmClasspathRoots
import org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.fir.extensions.FirAnalysisHandlerExtension

@OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)
class StubsCodegenCompilerPluginRegistrar : CompilerPluginRegistrar() {

  override val pluginId: String = "com.facebook.kotlin.compilerplugins.kosabi.stubsgen"

  override fun ExtensionStorage.registerExtensions(configuration: CompilerConfiguration) {

    val stubsGenDir: File = requireNotNull(configuration.get(stubsGenDirKey)?.let { File(it) })
    val stubsClassOutputDir: File =
        requireNotNull(configuration.get(stubsClassOutputDirKey)?.let { File(it) })

    // Register logger path if set
    Logger.userDefinedPath = configuration.get(stubsGenLogEnabledKey)
    val classPaths = configuration.jvmClasspathRoots
    Logger.log("[ClassPaths]\n")
    classPaths.forEach { Logger.log(it.canonicalPath) }

    FirAnalysisHandlerExtension.registerExtension(
        StubsCodegenK2FirAnalysisHandlerExtension(
            stubsDumpDir = stubsGenDir,
            stubsClassOutputDir = stubsClassOutputDir,
            classPaths = classPaths,
            skipJavaImports = configuration.get(skipJavaImportsKey).toBoolean(),
        ),
    )
  }

  override val supportsK2: Boolean
    get() = true
}
