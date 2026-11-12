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

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext
import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.jvm.core.BuildTargetValue
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
import com.facebook.buck.jvm.kotlin.cd.analytics.SourceTokenCounter
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode
import com.facebook.buck.util.ClassLoaderCache
import com.google.common.collect.ImmutableList
import com.google.common.collect.ImmutableSortedSet
import java.io.IOException
import java.io.PrintStream
import java.nio.file.Path
import java.util.Optional
import java.util.UUID
import kotlin.io.path.extension
import org.jetbrains.kotlin.buildtools.api.CompilationService
import org.jetbrains.kotlin.buildtools.api.ExperimentalBuildToolsApi
import org.jetbrains.kotlin.buildtools.api.ProjectId

/**
 * Entry point for Kotlin compilation using BuildToolsApi. See:
 * https://github.com/JetBrains/kotlin/blob/master/compiler/build-tools/kotlin-build-tools-api/README.md
 */
@OptIn(ExperimentalBuildToolsApi::class)
class BuildToolsKotlinc : Kotlinc {

  // Legacy BTA API is DEPRECATION_ERROR from Kotlin 2.4; KotlinToolchains replaces it.
  @Suppress("DEPRECATION_ERROR")
  override fun buildWithClasspath(
      context: IsolatedExecutionContext,
      invokingRule: BuildTargetValue,
      options: ImmutableList<String>,
      kotlinHomeLibraries: ImmutableList<AbsPath>,
      kotlinSourceFilePaths: ImmutableSortedSet<RelPath>,
      pathToSrcsList: Path,
      workingDirectory: Optional<Path>,
      ruleCellRoot: AbsPath,
      mode: KotlincMode,
      kotlinCDLoggingContext: KotlinCDLoggingContext,
      outputDirectory: AbsPath,
      classToSourceMapPath: Path?,
  ): Int {
    val argsStart = System.nanoTime()
    val (compilerArgs, expandedSources) =
        buildCompilerArgs(
            ruleCellRoot,
            kotlinSourceFilePaths,
            workingDirectory,
            invokingRule,
            options,
            kotlinCDLoggingContext,
        )
    val argsMs = (System.nanoTime() - argsStart) / 1_000_000

    LOG.info(
        "[KotlinC Toolchain Build Step from for target:${invokingRule.fullyQualifiedName} type:${invokingRule.type}] " +
            "Running ${CompilationService::class.java.name} ${getIncrementalInfoMessage(mode)} " +
            "with arguments:[${compilerArgs.joinToString()}] ",
    )

    // Machine-parseable log entry for tooling
    // Format: KOTLINCD_INVOCATION|target|type|incremental|arg_count
    // Followed by: KOTLINCD_ARG|<arg> for each argument
    LOG.info(
        "KOTLINCD_INVOCATION|${invokingRule.fullyQualifiedName}|${invokingRule.type}|${getIncrementalInfoMessage(mode)}|${compilerArgs.size}",
    )
    compilerArgs.forEach { arg -> LOG.info("KOTLINCD_ARG|$arg") }

    val classLoaderStart = System.nanoTime()
    val classLoader = context.classLoaderCache.getClassLoader(kotlinHomeLibraries)
    val classLoaderMs = (System.nanoTime() - classLoaderStart) / 1_000_000

    val serviceLoadStart = System.nanoTime()
    val kotlinCompilationService = KotlinCompilationService(
        CompilationService.loadImplementation(classLoader),
        kotlinCDLoggingContext,
    )
    val serviceLoadMs = (System.nanoTime() - serviceLoadStart) / 1_000_000

    val gcBefore = getGcStats()
    val compileStart = System.nanoTime()
    val result =
        kotlinCompilationService.compile(
            ProjectId.ProjectUUID(UUID.randomUUID()),
            compilerArgs,
            mode,
            BuckKotlinLogger(UncloseablePrintStream(context.stdErr), kotlinCDLoggingContext),
            classToSourceMapPath?.let {
              KotlinClassToSourceMap(
                  it,
                  outputDirectory.path,
                  ruleCellRoot.path,
                  (mode as? KotlincMode.Incremental)
                      ?.kotlicWorkingDir
                      ?.path
                      ?.resolve(
                          com.facebook.buck.jvm.java.CompilerClassToSourceMap.KOTLIN_FILE_NAME,
                      ),
              )
            },
        )
    val compileMs = (System.nanoTime() - compileStart) / 1_000_000
    val gcAfter = getGcStats()
    val gcDeltaMs = gcAfter.first - gcBefore.first
    val gcDeltaCount = gcAfter.second - gcBefore.second

    // After compiling, so that counting uses lexer classes the compiler has already loaded.
    val tokenCountStart = System.nanoTime()
    try {
      val tokenCounts =
          SourceTokenCounter.countTokens(expandedSources, ruleCellRoot.path, classLoader)
      kotlinCDLoggingContext.numKotlinTokens = tokenCounts.kotlinTokens
      kotlinCDLoggingContext.numJavaTokens = tokenCounts.javaTokens
    } catch (e: Exception) {
      // Token counting is best-effort telemetry; don't fail the build
      kotlinCDLoggingContext.addExtras(
          BuildToolsKotlinc::class.java.simpleName,
          "Token counting failed: ${e.cause ?: e}",
      )
    }
    val tokenCountMs = (System.nanoTime() - tokenCountStart) / 1_000_000

    LOG.info(
        "KOTLINCD_TIMING|${invokingRule.fullyQualifiedName}|args_ms=$argsMs|classloader_ms=$classLoaderMs|service_load_ms=$serviceLoadMs|compile_ms=$compileMs|gc_time_ms=$gcDeltaMs|gc_count=$gcDeltaCount|token_count_ms=$tokenCountMs",
    )

    return result.toExitCode.code
  }

  private fun getGcStats(): Pair<Long, Long> {
    var totalMs = 0L
    var totalCount = 0L
    for (bean in java.lang.management.ManagementFactory.getGarbageCollectorMXBeans()) {
      val time = bean.collectionTime
      val count = bean.collectionCount
      if (time >= 0) totalMs += time
      if (count >= 0) totalCount += count
    }
    return Pair(totalMs, totalCount)
  }

  private fun getIncrementalInfoMessage(mode: KotlincMode) =
      when (mode) {
        is KotlincMode.Incremental -> "incrementally"
        is KotlincMode.NonIncremental -> "non incrementally"
        else -> throw UnsupportedOperationException("Unsupported KotlincMode")
      }

  private fun ClassLoaderCache.getClassLoader(kotlinHomeLibraries: List<AbsPath>): ClassLoader {
    val classPathURLs = kotlinHomeLibraries.map { absPath -> absPath.path.toUri().toURL() }
    return getClassLoaderForClassPath(
        SharedApiClassesClassLoaderProvider.sharedApiClassesClassLoader,
        ImmutableList.copyOf(classPathURLs),
    )
  }

  /** Returns the compiler arguments and the expanded source paths. */
  private fun buildCompilerArgs(
      ruleCellRoot: AbsPath,
      kotlinSourceFilePaths: ImmutableSortedSet<RelPath>,
      workingDirectory: Optional<Path>,
      invokingRule: BuildTargetValue,
      options: List<String>,
      kotlinCDLoggingContext: KotlinCDLoggingContext,
  ): Pair<List<String>, List<Path>> {
    val expandStart = System.nanoTime()
    val expandedSources: ImmutableList<Path> = getExpandedSourcePathsOrThrow(
        ruleCellRoot,
        kotlinSourceFilePaths,
        workingDirectory,
        invokingRule,
    )
    val expandMs = (System.nanoTime() - expandStart) / 1_000_000
    if (expandMs > 100) {
      LOG.info(
          "KOTLINCD_SOURCE_EXPAND|${invokingRule.fullyQualifiedName}|expand_ms=$expandMs|source_count=${expandedSources.size}",
      )
    }

    expandedSources
        .groupingBy { path -> path.extension }
        .eachCount()
        .forEach { (extension, count) ->
          kotlinCDLoggingContext.addExtras(
              BuildToolsKotlinc::class.java.simpleName,
              "Total count of $extension files: $count",
          )
        }

    val resolvedExpandedSources = expandedSources.map { path ->
      ruleCellRoot.resolve(path).toString()
    }

    val isMultiPlatform = options.contains("-Xmulti-platform")

    return buildList {
      // Kotlin Multiplatform parameters pass-in source paths for each fragment
      // Similar to main source paths, we need to expand them as well
      if (isMultiPlatform) {
        val expandedOptions =
            getExpandedMultiPlatformSourcePathsOrThrow(options, resolvedExpandedSources)
        addAll(expandedOptions)
      } else {
        addAll(options)
      }

      addAll(resolvedExpandedSources)
    } to expandedSources
  }

  private fun getExpandedSourcePathsOrThrow(
      ruleCellRoot: AbsPath,
      kotlinSourceFilePaths: ImmutableSortedSet<RelPath>,
      workingDirectory: Optional<Path>,
      invokingRule: BuildTargetValue,
  ) =
      try {
        getExpandedSourcePaths(ruleCellRoot, kotlinSourceFilePaths, workingDirectory)
      } catch (exception: IOException) {
        LOG.error(exception)
        throw RuntimeException(
            "Unable to expand sources for ${invokingRule.fullyQualifiedName} into $workingDirectory",
        )
      }

  private fun getExpandedMultiPlatformSourcePathsOrThrow(
      options: List<String>,
      allSources: List<String>,
  ): List<String> =
      buildList() {
        options
            .filter { !it.isNullOrBlank() }
            .forEach { option ->
              if (option.startsWith("-Xfragment-sources")) {
                val (fragmentSourcesKey, unresolvedSources) = option.split("=")
                // construct `fragmentName:absolutePath` for each fragment source path
                // e.g. `-Xfragment-sources=android:/abs/path/A.kt,common:/abs/path/B.kt`
                val resolvedSourceOption =
                    unresolvedSources.split(",").map { fragmentSourcePath ->
                      val (fragmentName, fragmentPath) = fragmentSourcePath.split(":")
                      val fragmentSourceAbsPath = allSources.firstOrNull {
                        it.endsWith(fragmentPath)
                      }

                      if (fragmentSourceAbsPath == null) {
                        throw RuntimeException("Invalid fragment source path: $fragmentSourcePath")
                      }
                      "$fragmentName:$fragmentSourceAbsPath"
                    }
                add("$fragmentSourcesKey=${resolvedSourceOption.joinToString(",")}")
              } else {
                add(option)
              }
            }
      }

  override fun getDescription(
      options: ImmutableList<String>,
      kotlinSourceFilePaths: ImmutableSortedSet<RelPath>,
      pathToSrcsList: Path,
  ): String = buildString {
    append("kotlinc ")
    append(options.joinToString(separator = " "))
    append(" @")
    append(pathToSrcsList)
  }

  override fun getShortName(): String = "kotlinc"

  companion object {
    private val LOG: Logger = Logger.get(BuildToolsKotlinc::class.java)
  }
}

private class UncloseablePrintStream(delegate: PrintStream) : PrintStream(delegate) {
  override fun close() {
    // ignore
  }
}
