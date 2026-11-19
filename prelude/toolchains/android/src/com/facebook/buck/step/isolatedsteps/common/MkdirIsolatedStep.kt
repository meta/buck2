/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.step.isolatedsteps.common

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext
import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils
import com.facebook.buck.step.StepExecutionResult
import com.facebook.buck.step.StepExecutionResults
import com.facebook.buck.step.isolatedsteps.IsolatedStep
import com.facebook.buck.util.Escaper
import java.io.IOException
import java.nio.file.Files

/** Command that runs equivalent command of `mkdir -p` on the specified directory. */
data class MkdirIsolatedStep(val dirPath: RelPath) : IsolatedStep {
  @Throws(IOException::class)
  override fun executeIsolatedStep(context: IsolatedExecutionContext): StepExecutionResult {
    Files.createDirectories(
        ProjectFilesystemUtils.getPathForRelativePath(context.ruleCellRoot, dirPath.path),
    )
    return StepExecutionResults.SUCCESS
  }

  override fun getIsolatedStepDescription(context: IsolatedExecutionContext): String {
    return String.format("mkdir -p %s", Escaper.escapeAsShellString(dirPath.toString()))
  }

  override fun getShortName(): String {
    return "mkdir"
  }
}
