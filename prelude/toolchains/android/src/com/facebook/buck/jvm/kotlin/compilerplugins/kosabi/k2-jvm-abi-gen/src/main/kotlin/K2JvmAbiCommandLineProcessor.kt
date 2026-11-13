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

import org.jetbrains.kotlin.compiler.plugin.AbstractCliOption
import org.jetbrains.kotlin.compiler.plugin.CliOption
import org.jetbrains.kotlin.compiler.plugin.CliOptionProcessingException
import org.jetbrains.kotlin.compiler.plugin.CommandLineProcessor
import org.jetbrains.kotlin.config.CompilerConfiguration

@OptIn(org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi::class)
@SuppressWarnings("PackageLocationMismatch")
class K2JvmAbiCommandLineProcessor : CommandLineProcessor {
  companion object {
    const val COMPILER_PLUGIN_ID: String = "com.facebook.k2.jvm.abi.gen"

    val OUTPUT_PATH_OPTION: CliOption = CliOption(
        "outputDir",
        "<path>",
        "Output path for generated files. This can be either a directory or a jar file.",
        true,
    )

    val EARLY_TERMINATION_OPTION: CliOption = CliOption(
        "earlyTermination",
        "<boolean>",
        "Whether we should terminate K2JvmAbiGen early. Default is false.",
        required = false,
    )

    val ENABLE_MIXED_COMPILATION: CliOption = CliOption(
        "enableMixedCompilation",
        "<boolean>",
        "Enable to translate Java sources into ABI. Default is false.",
        false,
    )

    val DEBUG_OUTPUT_OPTION: CliOption = CliOption(
        "debugOutput",
        "<boolean>",
        "Enable debug output for K2 ABI generation. Default is false.",
        false,
    )

    const val COMPOSE_PLUGIN_PATH_FRAGMENT: String = "compose-compiler-plugin"

    val ENABLE_COMPOSE_ABI_EMULATION_OPTION: CliOption = CliOption(
        "enableComposeAbiEmulation",
        "<boolean>",
        "Enable Compose ABI emulation. Default is false.",
        false,
    )

    val AST_PACKAGE_NAMES_OPTION: CliOption = CliOption(
        "astPackageNames",
        "<boolean>",
        "Read each source file's package from its AST instead of building a PSI stub tree for " +
            "it. Default is true; false reads it through the stub tree as before.",
        false,
    )

    val ABI_VALIDATION_MODE_OPTION: CliOption = CliOption(
        "abiValidationMode",
        "off|warn|error",
        "How to report ABI content that had to be repaired because a dependency was not on " +
            "the source-only ABI classpath. `warn` reports each repair as a compiler warning, " +
            "`error` fails the compilation instead of emitting a knowingly-wrong ABI. " +
            "Default is off: fbsource builds Kotlin with `-Werror`, so `warn` also fails the " +
            "compile and has to be opted into per target.",
        false,
    )

    val SHALLOW_CLASS_USAGE_OPTION: CliOption = CliOption(
        "shallowClassUsage",
        "<boolean>",
        "Record only directly-referenced classes in dep files, recursing into supertypes " +
            "for the target's own supertype chain only. Default is false.",
        false,
    )

    val SKIP_NON_ABI_BODIES_OPTION: CliOption = CliOption(
        "skipNonAbiBodies",
        "<boolean>",
        "Skip resolving the function, accessor, constructor and init bodies that the " +
            "source-only ABI never reads. Default is true; false restores full body resolution.",
        false,
    )

    val KEPT_BODY_STUB_CHECK_OPTION: CliOption = CliOption(
        "keptBodyStubCheck",
        "<boolean>",
        "Fail the source-only ABI when a kept inline body names a class that only a Kosabi " +
            "stub provides. Default is true; false turns the check off.",
        false,
    )
  }

  override val pluginId: String
    get() = COMPILER_PLUGIN_ID

  override val pluginOptions: Collection<CliOption>
    get() = listOf(
        OUTPUT_PATH_OPTION,
        EARLY_TERMINATION_OPTION,
        ENABLE_MIXED_COMPILATION,
        DEBUG_OUTPUT_OPTION,
        ENABLE_COMPOSE_ABI_EMULATION_OPTION,
        AST_PACKAGE_NAMES_OPTION,
        ABI_VALIDATION_MODE_OPTION,
        SHALLOW_CLASS_USAGE_OPTION,
        SKIP_NON_ABI_BODIES_OPTION,
        KEPT_BODY_STUB_CHECK_OPTION,
    )

  override fun processOption(
      option: AbstractCliOption,
      value: String,
      configuration: CompilerConfiguration,
  ) {
    when (option) {
      OUTPUT_PATH_OPTION -> configuration.put(K2JvmAbiConfigurationKeys.OUTPUT_PATH, value)
      EARLY_TERMINATION_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.EARLY_TERMINATION, value == "true")
      ENABLE_MIXED_COMPILATION ->
          configuration.put(K2JvmAbiConfigurationKeys.ENABLE_MIXED_COMPILATION, value == "true")
      DEBUG_OUTPUT_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.DEBUG_OUTPUT, value == "true")
      ENABLE_COMPOSE_ABI_EMULATION_OPTION ->
          configuration.put(
              K2JvmAbiConfigurationKeys.ENABLE_COMPOSE_ABI_EMULATION,
              value == "true",
          )
      AST_PACKAGE_NAMES_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.AST_PACKAGE_NAMES, value != "false")
      ABI_VALIDATION_MODE_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.ABI_VALIDATION_MODE, value)
      SHALLOW_CLASS_USAGE_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.SHALLOW_CLASS_USAGE, value == "true")
      SKIP_NON_ABI_BODIES_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.SKIP_NON_ABI_BODIES, value != "false")
      KEPT_BODY_STUB_CHECK_OPTION ->
          configuration.put(K2JvmAbiConfigurationKeys.KEPT_BODY_STUB_CHECK, value != "false")
      else -> throw CliOptionProcessingException("Unknown option: ${option.optionName}")
    }
  }
}
