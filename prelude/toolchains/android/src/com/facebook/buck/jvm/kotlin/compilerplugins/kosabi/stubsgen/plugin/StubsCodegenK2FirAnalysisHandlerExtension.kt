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

import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators.StubsGenAPI // @oss-enable
// @oss-disable: import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators.meta_only.StubsGenApiImpl
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.parseJavaDeclarations
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.parseJavaSources
import java.io.File
import org.jetbrains.kotlin.cli.common.config.kotlinSourceRoots
import org.jetbrains.kotlin.cli.jvm.compiler.createSourceFilesFromSourceRoots
import org.jetbrains.kotlin.cli.jvm.config.javaSourceRoots
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.fir.extensions.FirAnalysisHandlerExtension

@SuppressWarnings("PackageLocationMismatch")
class StubsCodegenK2FirAnalysisHandlerExtension(
    private val stubsDumpDir: File,
    private val stubsClassOutputDir: File,
    private val classPaths: List<File>,
    private val skipJavaImports: Boolean = false,
) : FirAnalysisHandlerExtension() {

  override fun isApplicable(configuration: CompilerConfiguration): Boolean {
    return true
  }

  override fun doAnalysis(project: Project, configuration: CompilerConfiguration): Boolean {
    // Use the compiler's canonical source collection, which recognizes Kotlin sources by file
    // type (including .kts) and reports an error for missing source roots. Mirrors the approach
    // used by K2JvmAbiFirAnalysisHandlerExtension and replaces the deprecated
    // KotlinCoreEnvironment-based K1 source lookup.
    val ktFiles =
        createSourceFilesFromSourceRoots(configuration, project, configuration.kotlinSourceRoots)
    // Java sources ride the same kotlinc invocation but the Kotlin collector skips them:
    // [javaSourceRoots] are the raw roots passed to kotlinc, [kotlinSourceRoots] the parsed
    // Kotlin-only subset. Mixed modules need their Java imports stubbed too, or KSP-side
    // resolution (e.g. Ultralight body-call keys) fails.
    // Without KSP processors nothing reads Java imports, so skip the javac parse and collect
    // declarations only; the declared types still suppress duplicate same-module stubs.
    val javaSources =
        configuration.javaSourceRoots
            .map { File(it) }
            .flatMap { root ->
              when {
                root.isFile && root.extension == "java" -> listOf(root)
                root.isDirectory ->
                    root.walkTopDown().filter { it.isFile && it.extension == "java" }.toList()
                else -> emptyList<File>()
              }
            }
            .sortedBy { it.absolutePath }
            .let { if (skipJavaImports) parseJavaDeclarations(it) else parseJavaSources(it) }
    StubsGenAPI( // @oss-enable
    // @oss-disable: StubsGenApiImpl(
        stubsDumpDir,
        stubsClassOutputDir,
        classPaths,
        // @oss-disable: ktFiles,
    )
        .generateStubs(ktFiles, configuration, project, javaSources)

    throw RuntimeException("Terminating compilation. We're done with Stubgen.")

    return true
  }
}
