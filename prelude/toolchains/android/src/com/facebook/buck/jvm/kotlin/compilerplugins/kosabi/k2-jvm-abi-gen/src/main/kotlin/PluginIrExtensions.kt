/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook

import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration

/**
 * Registered plugin [IrGenerationExtension]s that abi-gen runs itself.
 *
 * Kosabi skips the standard backend, so plugin IR extensions never run in the source-only ABI
 * action and their ABI-visible effects are missing unless abi-gen reproduces them. A plugin listed
 * here has an extension that is safe on abi-gen's body-less IR and needs no emulation: abi-gen runs
 * the plugin's own extension, configured exactly as in the full compilation. Plugins not listed
 * stay skipped.
 *
 * Entries are exact class names; the extension is registered only when its plugin is on the
 * compilation, so listing a plugin does not enable it.
 */
internal val ABI_RELEVANT_PLUGIN_IR_EXTENSIONS: Set<String> = setOf(
    // DataClassGenerate replaces a data class's `Any` superclass with `DataClassSuper`.
    "com.facebook.kotlin.compilerplugins.dataclassgenerate.DataClassGenerateIrGenerationExtension",
)

internal fun abiRelevantPluginIrExtensions(
    project: Project,
    configuration: CompilerConfiguration,
): List<IrGenerationExtension> =
    registeredIrGenerationExtensionsCompat(project, configuration).filter {
      it.javaClass.name in ABI_RELEVANT_PLUGIN_IR_EXTENSIONS
    }
