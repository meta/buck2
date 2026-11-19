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

import org.jetbrains.kotlin.compiler.plugin.AbstractCliOption
import org.jetbrains.kotlin.compiler.plugin.CliOption
import org.jetbrains.kotlin.compiler.plugin.CliOptionProcessingException
import org.jetbrains.kotlin.compiler.plugin.CommandLineProcessor
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.config.CompilerConfigurationKey

private const val stubsGenDirName = "stubsgen-dir"
private const val stubsClassOutputDirName = "stubs-class-dir"
private const val stubsGenLogEnabledName = "stubsgen-log-enabled"
private const val skipJavaImportsName = "skip-java-imports"

val stubsGenDirKey: CompilerConfigurationKey<String> =
    CompilerConfigurationKey.create<String>("plugin $stubsGenDirName")

val stubsClassOutputDirKey: CompilerConfigurationKey<String> =
    CompilerConfigurationKey.create<String>("plugin $stubsClassOutputDirName")

val stubsGenLogEnabledKey: CompilerConfigurationKey<String> =
    CompilerConfigurationKey.create<String>("plugin $stubsGenLogEnabledName")

val skipJavaImportsKey: CompilerConfigurationKey<String> =
    CompilerConfigurationKey.create<String>("plugin $skipJavaImportsName")

@OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)
class StubsCodegenCommandLineProcessor : CommandLineProcessor {
  override val pluginId: String
    get() = COMPILER_PLUGIN_ID

  override val pluginOptions: Collection<AbstractCliOption>
    get() = listOf(STUBS_GEN_DIR, STUBS_CLASS_OUTPUT_DIR, LOG_ENABLED, SKIP_JAVA_IMPORTS)

  override fun processOption(
      option: AbstractCliOption,
      value: String,
      configuration: CompilerConfiguration,
  ) {
    when (option.optionName) {
      stubsGenDirName -> configuration.put(stubsGenDirKey, value)
      stubsClassOutputDirName -> configuration.put(stubsClassOutputDirKey, value)
      stubsGenLogEnabledName -> configuration.put(stubsGenLogEnabledKey, value)
      skipJavaImportsName -> configuration.put(skipJavaImportsKey, value)
      else -> throw CliOptionProcessingException("Unknown option: ${option.optionName}")
    }
  }

  companion object {
    const val COMPILER_PLUGIN_ID: String = "com.facebook.kotlin.compilerplugins.kosabi.stubsgen_k2"

    val STUBS_GEN_DIR: CliOption = CliOption(
        optionName = stubsGenDirName,
        valueDescription = "<file-path>",
        description = "Path to stubs directory.",
        required = false,
        allowMultipleOccurrences = false,
    )
    val STUBS_CLASS_OUTPUT_DIR: CliOption = CliOption(
        optionName = stubsClassOutputDirName,
        valueDescription = "<file-path>",
        description = "Path to stubs class output directory.",
        required = false,
        allowMultipleOccurrences = false,
    )

    val LOG_ENABLED: CliOption = CliOption(
        optionName = stubsGenLogEnabledName,
        valueDescription = "<file-path>",
        description = "Enable logger to write logs in file. local only",
        required = false,
        allowMultipleOccurrences = false,
    )

    val SKIP_JAVA_IMPORTS: CliOption = CliOption(
        optionName = skipJavaImportsName,
        valueDescription = "<true|false>",
        description =
            "Skip Java import collection (no KSP processors); declared types still parsed.",
        required = false,
        allowMultipleOccurrences = false,
    )
  }
}
