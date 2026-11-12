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

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.cli

import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators.StubsGenAPI // @oss-enable
import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
// @oss-disable: import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators.meta_only.StubsGenApiImpl
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.parseJavaDeclarations
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.parseJavaSources
import java.io.File
import org.jetbrains.kotlin.config.CompilerConfiguration

object StubsGenCli {

  fun generate(
      sourceFiles: List<File>,
      classpath: List<File>,
      stubsDumpDir: File?,
      stubsClassOutputDir: File?,
      logPath: String? = null,
      skipJavaImports: Boolean = false,
  ) {
    // Save/restore the process-global logger path so it doesn't leak across same-JVM callers
    // (e.g. StandaloneCliCompareTest); the CLI path is unaffected since the process exits.
    val prevLogPath = Logger.userDefinedPath
    Logger.userDefinedPath = logPath
    try {
      StubsGenParseOnlyEnvironment().use { env ->
        val (javaFiles, kotlinFiles) = sourceFiles.partition { it.extension == "java" }
        val ktFiles = env.parse(kotlinFiles)
        val javaSources =
            if (skipJavaImports) parseJavaDeclarations(javaFiles) else parseJavaSources(javaFiles)
        // configuration is unused by generateStubs; empty is fine.
        StubsGenAPI( // @oss-enable
        // @oss-disable: StubsGenApiImpl(
            stubsDumpDir = stubsDumpDir,
            stubsClassOutputDir = stubsClassOutputDir,
            classPaths = classpath,
            // @oss-disable: knownSources = ktFiles,
        )
            .generateStubs(ktFiles, CompilerConfiguration(), env.project, javaSources)
      }
    } finally {
      Logger.userDefinedPath = prevLogPath
    }
  }
}
