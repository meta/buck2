/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.cd.model.java.BuildTargetValue.Type;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.TestExecutionContext;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import org.junit.Rule;
import org.junit.Test;

public class RokokoStubgenStepTest {
  @Rule public TemporaryPaths tmp = new TemporaryPaths();

  @Test
  public void reportsErrorWhenNoKotlinSources() throws Exception {
    AbsPath root = tmp.getRoot();

    StepExecutionResult result =
        newStep(null, sourcesOf()).executeIsolatedStep(TestExecutionContext.newInstance(root));

    assertEquals(1, result.getExitCode());
    assertTrue(result.getStderr().orElse("").contains("no .kt sources"));
  }

  @Test
  public void reportsErrorWhenStubgenSoIsNull() throws Exception {
    AbsPath root = tmp.getRoot();

    StepExecutionResult result =
        newStep(null, sourcesOf("src/A.kt"))
            .executeIsolatedStep(TestExecutionContext.newInstance(root));

    assertEquals(1, result.getExitCode());
    assertTrue(result.getStderr().orElse("").contains("rokoko_stubgen_so"));
  }

  @Test
  public void reportsErrorWhenStubgenSoIsMissing() throws Exception {
    AbsPath root = tmp.getRoot();

    StepExecutionResult result =
        newStep(root.resolve("missing.so"), sourcesOf("src/A.kt"))
            .executeIsolatedStep(TestExecutionContext.newInstance(root));

    assertEquals(1, result.getExitCode());
  }

  private static RokokoStubgenStep newStep(AbsPath stubgenSo, ImmutableSortedSet<RelPath> sources) {
    return new RokokoStubgenStep(
        new BuildTargetValue(Type.SOURCE_ONLY_ABI, "//foo:bar"),
        stubgenSo,
        sources,
        ImmutableList.of(),
        RelPath.get("stubs"),
        RelPath.get("classes"),
        RelPath.get("reports"));
  }

  private static ImmutableSortedSet<RelPath> sourcesOf(String... paths) {
    ImmutableSortedSet.Builder<RelPath> sources =
        ImmutableSortedSet.orderedBy(RelPath.comparator());
    for (String path : paths) {
      sources.add(RelPath.get(path));
    }
    return sources.build();
  }
}
