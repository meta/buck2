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
)

package com.facebook

import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.cli.jvm.compiler.VfsBasedProjectEnvironment
import org.jetbrains.kotlin.cli.jvm.compiler.createContextForIncrementalCompilation
import org.jetbrains.kotlin.cli.jvm.compiler.createLibraryListForJvm
import org.jetbrains.kotlin.cli.pipeline.jvm.JvmFrontendPipelinePhase
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.diagnostics.DiagnosticReporterFactory
import org.jetbrains.kotlin.diagnostics.impl.BaseDiagnosticsCollector
import org.jetbrains.kotlin.extensions.CompilerConfigurationExtension
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.declarations.FirNamedFunction
import org.jetbrains.kotlin.fir.pipeline.AllModulesFrontendOutput
import org.jetbrains.kotlin.fir.pipeline.SingleModuleFrontendOutput
import org.jetbrains.kotlin.fir.resolve.ScopeSession
import org.jetbrains.kotlin.fir.session.IncrementalCompilationContext
import org.jetbrains.kotlin.fir.session.environment.AbstractProjectFileSearchScope
import org.jetbrains.kotlin.fir.visitors.FirDefaultVisitorVoid
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.resolve.multiplatform.hmppModuleName
import org.jetbrains.kotlin.resolve.multiplatform.isCommonSource

/** Frontend analysis output wrapper. Was FirResult before Kotlin 2.3. */
typealias FrontendOutputCompat = AllModulesFrontendOutput

/** Per-module frontend output element. Was ModuleCompilerAnalyzedOutput before Kotlin 2.3. */
typealias ModuleFrontendOutputCompat = SingleModuleFrontendOutput

/** Source function declaration. Was FirSimpleFunction before Kotlin 2.3. */
typealias FirNamedFunctionCompat = FirNamedFunction

fun moduleFrontendOutputCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    fir: List<FirFile>,
): ModuleFrontendOutputCompat = SingleModuleFrontendOutput(session, scopeSession, fir)

fun frontendOutputCompat(outputs: List<ModuleFrontendOutputCompat>): FrontendOutputCompat =
    AllModulesFrontendOutput(outputs)

/**
 * FIR visitor base that bridges the function visit method renamed in Kotlin 2.3
 * (visitSimpleFunction -> visitNamedFunction). Subclasses implement [visitNamedFunctionCompat].
 */
abstract class FirNamedFunctionVisitorCompat : FirDefaultVisitorVoid() {
  final override fun visitNamedFunction(namedFunction: FirNamedFunction) {
    visitNamedFunctionCompat(namedFunction)
  }

  abstract fun visitNamedFunctionCompat(function: FirNamedFunctionCompat)
}

/** createPendingReporter dropped its MessageCollector parameter in Kotlin 2.3. */
fun createPendingReporterCompat(messageCollector: MessageCollector): BaseDiagnosticsCollector =
    DiagnosticReporterFactory.createPendingReporter()

/** ModuleCompilerOutput.generationState.factory was flattened to .factory in Kotlin 2.3. */
val ModuleCompilerOutputCompat.classFileFactoryCompat
  get() = factory

/** RETAIN_OUTPUT_IN_MEMORY is no longer required in Kotlin 2.3. */
fun CompilerConfiguration.retainOutputInMemoryCompat() {
  // No-op: output is no longer retained in memory in Kotlin 2.3.
}

/**
 * Prepares the FIR JVM session(s) for the given source files. Kotlin 2.3 replaced the manual
 * session-construction pipeline with JvmFrontendPipelinePhase.prepareJvmSessions.
 */
fun prepareJvmSessionsCompat(
    ktFiles: List<KtFile>,
    rootModuleName: String,
    friendPaths: List<String>,
    librariesScope: AbstractProjectFileSearchScope,
    configuration: CompilerConfiguration,
    projectEnvironment: VfsBasedProjectEnvironment,
    providerAndScopeForIncrementalCompilation: IncrementalCompilationContext?,
): List<Pair<FirSession, List<KtFile>>> {
  val libraryList = createLibraryListForJvm(rootModuleName, configuration, friendPaths)
  return JvmFrontendPipelinePhase.prepareJvmSessions(
          ktFiles,
          Name.special("<$rootModuleName>"),
          configuration,
          projectEnvironment,
          librariesScope,
          libraryList,
          isCommonSource = { false },
          isScript = { false },
          fileBelongsToModule = { file: KtFile, moduleName: String ->
            file.hmppModuleName == moduleName
          },
          createProviderAndScopeForIncrementalCompilation = {
            providerAndScopeForIncrementalCompilation
          },
      )
      .map { (session, sources) -> session to sources }
}

var KtFile.isCommonSourceCompat: Boolean?
  get() = isCommonSource
  set(value) {
    isCommonSource = value
  }

var KtFile.hmppModuleNameCompat: String?
  get() = hmppModuleName
  set(value) {
    hmppModuleName = value
  }

fun CompilerConfigurationExtension.updateFileRegistryCompat(project: Project) {
  updateFileRegistry()
}

/**
 * The project libraries scope, minus the incremental compilation output when there is one, and the
 * incremental compilation context.
 */
fun prepareLibrariesScopeAndIncrementalContextCompat(
    projectEnvironment: VfsBasedProjectEnvironment,
    configuration: CompilerConfiguration,
    ktFiles: List<KtFile>,
): Pair<AbstractProjectFileSearchScope, IncrementalCompilationContext?> {
  val sourceScope =
      projectEnvironment.getSearchScopeByPsiFiles(ktFiles) +
          projectEnvironment.getSearchScopeForProjectJavaSources()
  val context =
      createContextForIncrementalCompilation(projectEnvironment, configuration, sourceScope)
  var librariesScope = projectEnvironment.getSearchScopeForProjectLibraries()
  context?.precompiledBinariesFileScope?.let { librariesScope -= it }
  return librariesScope to context
}
