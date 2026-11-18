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
@file:OptIn(
    org.jetbrains.kotlin.K1Deprecation::class,
    org.jetbrains.kotlin.psi.KtImplementationDetail::class,
)

package com.facebook

import org.jetbrains.kotlin.cli.jvm.compiler.VfsBasedProjectEnvironment
import org.jetbrains.kotlin.cli.jvm.compiler.prepareIncrementalCompilationContextAndLibrariesClasspath
import org.jetbrains.kotlin.cli.pipeline.jvm.JvmFrontendPipelinePhase
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.session.IncrementalCompilationContext
import org.jetbrains.kotlin.jvm.environment.JvmClasspath
import org.jetbrains.kotlin.name.Name
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.psi.hmppModuleName

fun prepareLibrariesScopeAndIncrementalContextCompat(
    projectEnvironment: VfsBasedProjectEnvironment,
    configuration: CompilerConfiguration,
    ktFiles: List<KtFile>,
): Pair<JvmClasspath, IncrementalCompilationContext?> =
    prepareIncrementalCompilationContextAndLibrariesClasspath(configuration, projectEnvironment)

fun prepareJvmSessionsCompat(
    ktFiles: List<KtFile>,
    rootModuleName: String,
    friendPaths: List<String>,
    librariesScope: JvmClasspath,
    configuration: CompilerConfiguration,
    projectEnvironment: VfsBasedProjectEnvironment,
    providerAndScopeForIncrementalCompilation: IncrementalCompilationContext?,
): List<Pair<FirSession, List<KtFile>>> {
  val libraryList =
      JvmFrontendPipelinePhase.createLibraryListForJvm(rootModuleName, configuration, friendPaths)
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
          incrementalCompilationContext = providerAndScopeForIncrementalCompilation,
      )
      .map { (session, sources) -> session to sources }
}
