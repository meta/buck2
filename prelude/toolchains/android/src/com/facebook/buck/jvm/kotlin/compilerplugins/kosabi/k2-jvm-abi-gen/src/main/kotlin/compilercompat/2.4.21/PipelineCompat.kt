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

import org.jetbrains.kotlin.backend.common.actualizer.IrActualizedResult
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.jvm.JvmIrCodegenFactory
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.cli.extensionsStorage
import org.jetbrains.kotlin.cli.jvm.compiler.EnvironmentConfigFiles
import org.jetbrains.kotlin.cli.jvm.compiler.VfsBasedProjectEnvironment
import org.jetbrains.kotlin.cli.pipeline.jvm.JvmFir2IrPipelinePhase
import org.jetbrains.kotlin.cli.pipeline.jvm.JvmFrontendPipelinePhase
import org.jetbrains.kotlin.cli.pipeline.jvm.JvmWriteOutputsPhase
import org.jetbrains.kotlin.codegen.ClassBuilderFactories
import org.jetbrains.kotlin.codegen.state.GenerationState
import org.jetbrains.kotlin.com.intellij.openapi.Disposable
import org.jetbrains.kotlin.compiler.plugin.CompilerPluginRegistrar
import org.jetbrains.kotlin.compiler.plugin.ExperimentalCompilerApi
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.config.JVMConfigurationKeys
import org.jetbrains.kotlin.diagnostics.impl.BaseDiagnosticsCollector
import org.jetbrains.kotlin.fir.backend.Fir2IrComponents
import org.jetbrains.kotlin.fir.backend.Fir2IrPluginContext
import org.jetbrains.kotlin.fir.backend.jvm.FirJvmBackendClassResolver
import org.jetbrains.kotlin.fir.backend.jvm.FirJvmBackendExtension
import org.jetbrains.kotlin.fir.backend.jvm.JvmFir2IrExtensions
import org.jetbrains.kotlin.fir.backend.utils.extractFirDeclarations
import org.jetbrains.kotlin.fir.pipeline.AllModulesFrontendOutput
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.util.SymbolTable
import org.jetbrains.kotlin.modules.TargetId

// Kotlin 2.4 removed the legacy K2 CLI pipeline (`cli.jvm.compiler.legacy.pipeline`). These keep
// its
// data shapes and its `generateCodeFromIr` steps, built on the phases that replaced it.
data class ModuleCompilerEnvironmentCompat(
    val projectEnvironment: VfsBasedProjectEnvironment,
    val diagnosticsReporter: BaseDiagnosticsCollector,
)

data class ModuleCompilerIrBackendInputCompat(
    val targetId: TargetId,
    val configuration: CompilerConfiguration,
    val extensions: JvmFir2IrExtensions,
    val irModuleFragment: IrModuleFragment,
    val components: Fir2IrComponents,
    val pluginContext: Fir2IrPluginContext,
    val irActualizedResult: IrActualizedResult?,
    val symbolTable: SymbolTable,
)

// ast-grep-ignore: common/kotlin/no-new-typealias-warn
typealias ModuleCompilerOutputCompat = GenerationState

fun createJvmFir2IrExtensionsCompat(configuration: CompilerConfiguration) = JvmFir2IrExtensions()

fun AllModulesFrontendOutput.convertToIrAndActualizeForJvmCompat(
    fir2IrExtensions: JvmFir2IrExtensions,
    configuration: CompilerConfiguration,
    diagnosticsReporter: BaseDiagnosticsCollector,
    irGeneratorExtensions: Collection<IrGenerationExtension>,
) =
    with(JvmFir2IrPipelinePhase) {
      convertToIrAndActualizeForJvm(
          fir2IrExtensions,
          configuration,
          diagnosticsReporter,
          irGeneratorExtensions,
      )
    }

/**
 * Errors go to the configuration's message collector, the one callers pass as [messageCollector].
 *
 * Creating the project registers every plugin's extensions into the configuration's extension
 * storage, which FIR sessions read their extension registrars from. A copied configuration shares
 * that storage with the compilation running Kosabi, so it gets its own storage first; otherwise
 * every plugin's FIR extensions are registered twice.
 */
@OptIn(ExperimentalCompilerApi::class)
fun createProjectEnvironmentCompat(
    configuration: CompilerConfiguration,
    parentDisposable: Disposable,
    configFiles: EnvironmentConfigFiles,
    messageCollector: MessageCollector,
): VfsBasedProjectEnvironment {
  configuration.extensionsStorage = CompilerPluginRegistrar.ExtensionStorage()
  return JvmFrontendPipelinePhase.createProjectEnvironment(
      configuration,
      parentDisposable,
      configFiles,
  )
}

fun generateCodeFromIrCompat(
    irInput: ModuleCompilerIrBackendInputCompat,
    environment: ModuleCompilerEnvironmentCompat,
): GenerationState {
  replaceErrorTypesForMetadata(irInput.irModuleFragment)
  val generationState = GenerationState(
      environment.projectEnvironment.project,
      irInput.irModuleFragment.descriptor,
      irInput.configuration,
      ClassBuilderFactories.BINARIES,
      targetId = irInput.targetId,
      moduleName = irInput.targetId.name,
      jvmBackendClassResolver = FirJvmBackendClassResolver(irInput.components),
      diagnosticReporter = environment.diagnosticsReporter,
  )
  val backendInput =
      JvmIrCodegenFactory.BackendInput(
          irInput.irModuleFragment,
          irInput.pluginContext.irBuiltIns,
          irInput.symbolTable,
          irInput.components.irProviders,
          debuggerExtensions = null,
          FirJvmBackendExtension(
              irInput.components,
              irInput.irActualizedResult?.actualizedExpectDeclarations?.extractFirDeclarations(),
          ),
          irInput.pluginContext,
      )
  val codegenFactory = JvmIrCodegenFactory(irInput.configuration)
  codegenFactory.invokeCodegen(codegenFactory.invokeLowerings(generationState, backendInput))
  if (irInput.configuration[JVMConfigurationKeys.OUTPUT_DIRECTORY] != null) {
    JvmWriteOutputsPhase.writeOutputsIfNeeded(
        environment.projectEnvironment.project,
        irInput.configuration,
        environment.diagnosticsReporter.hasErrors,
        listOf(generationState),
        mainClassFqName = null,
    )
  }
  return generationState
}
