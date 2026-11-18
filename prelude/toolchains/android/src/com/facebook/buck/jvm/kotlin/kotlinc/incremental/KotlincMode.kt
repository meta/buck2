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

import com.facebook.buck.core.filesystems.AbsPath
import java.nio.file.Path

sealed interface KotlincMode {

  data object NonIncremental : KotlincMode

  data class Incremental
  @JvmOverloads
  constructor(
      val rootProjectDir: AbsPath,
      val buildDir: AbsPath,
      val kotlicWorkingDir: AbsPath,
      val kotlinSourceChanges: KotlinSourceChanges,
      val classpathChanges: ClasspathChanges,
      val kotlinClassUsageFile: AbsPath?,
      val rebuildReason: RebuildReason?,
      /** Null when kotlincd does not track the inputs in [IncrementalCompilerInputs]. */
      val inputTracking: IncrementalInputTracking? = null,
  ) : KotlincMode {

    /** This mode, compiling in full for [reason] unless it already compiles in full. */
    fun withRebuildReason(reason: RebuildReason): Incremental =
        if (rebuildReason == null) copy(rebuildReason = reason) else this
  }
}

/** What kotlincd needs to track [IncrementalCompilerInputs]. */
data class IncrementalInputTracking
@JvmOverloads
constructor(
    /** The action's input digests, by project-relative path. */
    val inputDigests: Map<Path, String>,
    /** The target's own compiler arguments and plugin option values, which may name files. */
    val optionValues: List<String> = emptyList(),
)
