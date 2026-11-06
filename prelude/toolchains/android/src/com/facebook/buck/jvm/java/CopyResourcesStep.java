/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.java;

import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MkdirIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.SymlinkIsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.util.Map;

/** Copies (by creating symlinks) resources from existing paths to desired paths. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class CopyResourcesStep {

  private CopyResourcesStep() {}

  /** Copies (by creating symlinks) resources from existing paths to desired paths. */
  public static ImmutableList<IsolatedStep> of(ImmutableMap<RelPath, RelPath> resources) {
    ImmutableList.Builder<IsolatedStep> steps =
        ImmutableList.builderWithExpectedSize(resources.size() * 2);
    for (Map.Entry<RelPath, RelPath> entry : resources.entrySet()) {
      RelPath existingPath = entry.getKey();
      RelPath linkPath = entry.getValue();

      steps.add(new MkdirIsolatedStep(linkPath.getParent()));
      steps.add(new SymlinkIsolatedStep(existingPath, linkPath));
    }
    return steps.build();
  }
}
