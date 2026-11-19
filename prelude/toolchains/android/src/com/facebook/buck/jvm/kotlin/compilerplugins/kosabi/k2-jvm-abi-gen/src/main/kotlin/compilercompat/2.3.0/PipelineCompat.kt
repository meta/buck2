/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress(
    "PackageLocationMismatch",
    "DEPRECATION",
    "DEPRECATION_ERROR",
    "TYPEALIAS_EXPANSION_DEPRECATION",
    "TYPEALIAS_EXPANSION_DEPRECATION_ERROR",
)
@file:OptIn(org.jetbrains.kotlin.cli.common.LegacyK2CliPipeline::class)

package com.facebook

import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.jvm.JvmIrDeserializerImpl
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.cli.jvm.compiler.EnvironmentConfigFiles
import org.jetbrains.kotlin.cli.jvm.compiler.legacy.pipeline.ModuleCompilerEnvironment
import org.jetbrains.kotlin.cli.jvm.compiler.legacy.pipeline.ModuleCompilerIrBackendInput
import org.jetbrains.kotlin.cli.jvm.compiler.legacy.pipeline.convertToIrAndActualizeForJvm
import org.jetbrains.kotlin.cli.jvm.compiler.legacy.pipeline.createProjectEnvironment
import org.jetbrains.kotlin.cli.jvm.compiler.legacy.pipeline.generateCodeFromIr
import org.jetbrains.kotlin.codegen.state.GenerationState
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.diagnostics.impl.BaseDiagnosticsCollector
import org.jetbrains.kotlin.fir.backend.jvm.JvmFir2IrExtensions
import org.jetbrains.kotlin.fir.pipeline.AllModulesFrontendOutput

@Suppress("DEPRECATION_ERROR")
typealias ModuleCompilerEnvironmentCompat = ModuleCompilerEnvironment

typealias ModuleCompilerIrBackendInputCompat = ModuleCompilerIrBackendInput

// generateCodeFromIr returns GenerationState directly in Kotlin 2.3 (the
// ModuleCompilerOutput wrapper was removed); .factory is exposed on it.
typealias ModuleCompilerOutputCompat = GenerationState

fun AllModulesFrontendOutput.convertToIrAndActualizeForJvmCompat(
    fir2IrExtensions: JvmFir2IrExtensions,
    configuration: CompilerConfiguration,
    diagnosticsReporter: BaseDiagnosticsCollector,
    irGeneratorExtensions: Collection<IrGenerationExtension>,
) = convertToIrAndActualizeForJvm(
    fir2IrExtensions,
    configuration,
    diagnosticsReporter,
    irGeneratorExtensions,
)

fun createProjectEnvironmentCompat(
    configuration: CompilerConfiguration,
    parentDisposable: org.jetbrains.kotlin.com.intellij.openapi.Disposable,
    configFiles: EnvironmentConfigFiles,
    messageCollector: MessageCollector,
) = createProjectEnvironment(configuration, parentDisposable, configFiles, messageCollector)

@Suppress("DEPRECATION_ERROR")
fun generateCodeFromIrCompat(
    irInput: ModuleCompilerIrBackendInputCompat,
    environment: ModuleCompilerEnvironmentCompat,
) = generateCodeFromIr(irInput, environment)

fun createJvmFir2IrExtensionsCompat(configuration: CompilerConfiguration) =
    JvmFir2IrExtensions(configuration, JvmIrDeserializerImpl())
