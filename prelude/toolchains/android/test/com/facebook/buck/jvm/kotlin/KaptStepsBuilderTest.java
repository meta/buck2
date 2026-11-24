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
import static org.mockito.ArgumentMatchers.anyString;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.core.BuildTargetValueExtraParams;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.JavacPluginParams;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics;
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;
import java.nio.file.Paths;
import java.util.Optional;
import org.junit.Test;

public class KaptStepsBuilderTest {
  @Test
  public void generatedClassesAreReturnedForCompilation() {
    ResolvedJavacPluginProperties processor = mock(ResolvedJavacPluginProperties.class);
    when(processor.getProcessorNames()).thenReturn(ImmutableSortedSet.of("com.example.Processor"));
    when(processor.getRunsOnJavaOnly()).thenReturn(false);
    when(processor.toUrlClasspath(AbsPath.get("/repo"))).thenReturn(ImmutableList.of());

    ImmutableList<AbsPath> classpaths = prepare(false, ImmutableList.of(processor));

    assertEquals(ImmutableList.of(AbsPath.get("/repo/__target_kapt_classes__")), classpaths);
  }

  @Test
  public void noProcessorsReturnNoClasspaths() {
    assertTrue(prepare(false, ImmutableList.of()).isEmpty());
  }

  @Test
  public void sourceOnlyAbiReturnsNoClasspaths() {
    assertTrue(
        prepare(true, ImmutableList.of(mock(ResolvedJavacPluginProperties.class))).isEmpty());
  }

  private static ImmutableList<AbsPath> prepare(
      boolean sourceOnlyAbi, ImmutableList<ResolvedJavacPluginProperties> processors) {
    BuildTargetValue rule = mock(BuildTargetValue.class);
    when(rule.isSourceOnlyAbi()).thenReturn(sourceOnlyAbi);
    BuildTargetValueExtraParams paths = mock(BuildTargetValueExtraParams.class);
    when(paths.getKaptAnnotationGenPath()).thenReturn(RelPath.get("staged"));
    when(paths.getAnnotationOutputPath(anyString()))
        .thenAnswer(call -> RelPath.get(String.format((String) call.getArgument(0), "target")));
    when(paths.getGenPath(anyString()))
        .thenAnswer(call -> RelPath.get(String.format((String) call.getArgument(0), "target")));

    return KaptStepsBuilder.prepareKaptProcessorsIfNeeded(
        rule,
        AbsPath.get("/repo"),
        ImmutableList.<IsolatedStep>builder(),
        paths,
        Optional.empty(),
        AbsPath.get("/repo/stdlib.jar"),
        AbsPath.get("/repo/kapt.jar"),
        RelPath.get("classes"),
        RelPath.get("annotations"),
        ImmutableSortedSet.orderedBy(RelPath.comparator()),
        RelPath.get("reports"),
        false,
        ImmutableList.<IsolatedStep>builder(),
        new JavacPluginParams(processors, ImmutableSortedSet.of()),
        ImmutableList.of(),
        ImmutableSortedSet.of(),
        Paths.get("sources.list"),
        ImmutableList.of(),
        ImmutableList.of(),
        mock(Kotlinc.class),
        mock(CompilerOutputPaths.class),
        RelPath.get("buck-out"),
        ImmutableMap.of(),
        "/repo/plugin-output",
        "test-module",
        mock(KotlinCDAnalytics.class),
        ImmutableSortedSet.orderedBy(RelPath.comparator()),
        ImmutableSortedSet.orderedBy(RelPath.comparator()),
        mock(LanguageVersion.class));
  }
}
