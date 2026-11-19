/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.buildtools.snapshot

import com.facebook.buck.core.util.log.Logger
import java.nio.file.Path
import kotlin.system.measureTimeMillis
import org.jetbrains.kotlin.buildtools.api.CompilationService
import org.jetbrains.kotlin.buildtools.api.ExperimentalBuildToolsApi

@OptIn(ExperimentalBuildToolsApi::class)
class ClasspathSnapshotGenerator(
    private val inputJar: Path,
    private val outputSnapshot: Path,
    private val granularity: SnapshotGranularity,
) {

  // Legacy BTA API is DEPRECATION_ERROR from Kotlin 2.4; KotlinToolchains replaces it.
  @Suppress("DEPRECATION_ERROR")
  fun run() {
    val timeSpent = measureTimeMillis {
      val compilationService = CompilationService.loadImplementation(this.javaClass.classLoader!!)
      val snapshot =
          compilationService.calculateClasspathSnapshot(
              inputJar.toFile(),
              granularity.toClassSnapshotGranularity,
          )
      snapshot.saveSnapshot(outputSnapshot.toFile())
    }
    LOG.info("Classpath snapshot generation took $timeSpent ms for input jar: $inputJar")
  }

  companion object {
    private val LOG: Logger = Logger.get(ClasspathSnapshotGenerator::class.java)
  }
}
