/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.step.isolatedsteps.java;

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.impl.ProjectFilesystemUtils;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.StepExecutionResults;
import com.facebook.buck.step.isolatedsteps.common.AbstractIsolatedExecutionStep;
import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;

/** Make Missing Output directories step. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class MakeMissingOutputsStep extends AbstractIsolatedExecutionStep {

  private final RelPath annotationsPath;

  public MakeMissingOutputsStep(RelPath annotationsPath) {
    super("make_missing_outputs");
    this.annotationsPath = annotationsPath;
  }

  @Override
  public StepExecutionResult executeIsolatedStep(IsolatedExecutionContext context)
      throws IOException {
    AbsPath root = context.getRuleCellRoot();
    if (!ProjectFilesystemUtils.exists(root, annotationsPath.getPath())) {
      ProjectFilesystemUtils.mkdirs(root, annotationsPath.getPath());
    }
    return StepExecutionResults.SUCCESS;
  }
}
