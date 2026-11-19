/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.analytics

import com.facebook.buck.core.filesystems.AbsPath

enum class StepParam(val value: String) {
  KOTLINC("kotlinc"),
  KOSABI_STUBGEN("kosabi_stubgen"),
  KSP2("ksp2"),
  KAPT("kapt"),
}

sealed class ModeParam(val value: String) {

  data class Incremental(
      val classpathChangesParam: ClasspathChangesParam,
      // Null set indicates that the files are unknown, while empty set indicates that there are no
      // files
      val addedAndModifiedFiles: Set<AbsPath>? = null,
      val removedFiles: Set<AbsPath>? = null,
  ) : ModeParam("incremental")

  data object NonIncremental : ModeParam("non_incremental")
}

enum class ClasspathChangesParam(val value: String) {
  TO_BE_COMPUTED_BY_INCREMENTAL_COMPILER("to_be_computed_by_incremental_compiler"),
  NO_CHANGES("no_changes"),
  UNKNOWN("unknown"),
}
