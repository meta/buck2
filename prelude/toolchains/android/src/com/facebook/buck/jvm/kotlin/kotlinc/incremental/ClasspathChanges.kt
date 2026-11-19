/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.kotlinc.incremental

import java.io.File

/**
 * Changes to the classpath of the KotlinC action, or information to compute them later by the
 * Kotlin incremental compiler
 */
sealed interface ClasspathChanges {

  val classpathSnapshotFiles: List<File>

  /**
   * Classpath has changes (additions, modifications, or removals). The Kotlin incremental compiler
   * computes affected sources.
   */
  data class ToBeComputedByIncrementalCompiler(override val classpathSnapshotFiles: List<File>) :
      ClasspathChanges

  data class NoChanges(override val classpathSnapshotFiles: List<File>) : ClasspathChanges

  @Deprecated("For testing only, will be removed.")
  data object Unknown : ClasspathChanges {
    override val classpathSnapshotFiles: List<File> = emptyList()
  }
}
