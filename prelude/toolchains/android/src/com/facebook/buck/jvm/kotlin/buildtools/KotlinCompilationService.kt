/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.buildtools

import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode
import org.jetbrains.kotlin.buildtools.api.CompilationResult
import org.jetbrains.kotlin.buildtools.api.CompilationService
import org.jetbrains.kotlin.buildtools.api.ExperimentalBuildToolsApi
import org.jetbrains.kotlin.buildtools.api.KotlinLogger
import org.jetbrains.kotlin.buildtools.api.ProjectId

// Legacy BTA API is DEPRECATION_ERROR from Kotlin 2.4; KotlinToolchains replaces it.
@Suppress("DEPRECATION_ERROR")
@OptIn(ExperimentalBuildToolsApi::class)
internal class KotlinCompilationService(
    private val compilationService: CompilationService,
    kotlinCDLoggingContext: KotlinCDLoggingContext,
) {

  private val jvmCompilationConfigurationFactory =
      JvmCompilationConfigurationFactory(compilationService, kotlinCDLoggingContext)

  fun compile(
      projectId: ProjectId,
      arguments: List<String>,
      mode: KotlincMode,
      logger: KotlinLogger? = null,
      classToSourceMap: KotlinClassToSourceMap? = null,
  ): CompilationResult {
    val compilerExecutionStrategyConfiguration =
        compilationService.makeCompilerExecutionStrategyConfiguration()

    val jvmCompilationConfiguration =
        jvmCompilationConfigurationFactory
            .create(mode, classToSourceMap?.requiresRebuild ?: false)
            .apply {
              if (logger != null) {
                useLogger(logger)
              }
              if (classToSourceMap != null) {
                useLogger(classToSourceMap.logger(this.logger))
              }
            }

    val result =
        compilationService.compileJvm(
            projectId,
            compilerExecutionStrategyConfiguration,
            jvmCompilationConfiguration,
            emptyList(),
            if (classToSourceMap != null) arguments + "-Xreport-output-files" else arguments,
        )
    if (result == CompilationResult.COMPILATION_SUCCESS) classToSourceMap?.write()
    return result
  }
}
