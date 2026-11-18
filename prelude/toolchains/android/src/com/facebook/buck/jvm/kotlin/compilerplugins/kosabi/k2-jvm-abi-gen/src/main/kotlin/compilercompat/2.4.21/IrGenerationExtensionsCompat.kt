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
import org.jetbrains.kotlin.compiler.plugin.getCompilerExtensions
import org.jetbrains.kotlin.config.CompilerConfiguration

/** Kotlin 2.4 reads IR extensions from the configuration's extension storage, not the project. */
fun registeredIrGenerationExtensionsCompat(
    @Suppress("UNUSED_PARAMETER") project: Project,
    configuration: CompilerConfiguration,
): List<IrGenerationExtension> = configuration.getCompilerExtensions(IrGenerationExtension)
