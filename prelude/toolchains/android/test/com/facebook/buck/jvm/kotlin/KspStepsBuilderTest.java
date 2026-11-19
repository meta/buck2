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
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.CopyIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.RmIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.ZipIsolatedStep;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import org.junit.Test;

public class KspStepsBuilderTest {

  private static final String METAGEN_PROCESSOR =
      "KSP:com.facebook.metagen.processor.kspmetagen.MetagenKspProcessorProvider";

  @Test
  public void usesFinalRoundPlaceholder_forEverySentinelProducer() {
    ImmutableList<String> processorNames =
        ImmutableList.of(
            "KSP:com.facebook.annotationprocessors.inject.ksp.InjectorKspProcessorProvider",
            METAGEN_PROCESSOR);

    for (String processorName : processorNames) {
      assertTrue(
          processorName,
          KspStepsBuilder.usesFinalRoundPlaceholder(
              ImmutableList.of(processorWithNames(processorName))));
    }
  }

  @Test
  public void usesFinalRoundPlaceholder_findsLaterProcessorName() {
    assertTrue(
        KspStepsBuilder.usesFinalRoundPlaceholder(
            ImmutableList.of(
                processorWithNames("KSP:com.example.UnrelatedProcessor"),
                processorWithNames("KSP:com.example.OtherProcessor", METAGEN_PROCESSOR))));
  }

  @Test
  public void usesFinalRoundPlaceholder_ignoresRetiredAdapterProcessors() {
    for (String processorName :
        ImmutableList.of(
            "KSP:com.facebook.annotationprocessors.gatekeepers.ksp.GatekeeperDeclarationKspProcessor",
            "KSP:com.facebook.annotationprocessors.qe.ksp.QEKspProcessorProvider")) {
      assertFalse(
          processorName,
          KspStepsBuilder.usesFinalRoundPlaceholder(
              ImmutableList.of(processorWithNames(processorName))));
    }
  }

  @Test
  public void usesFinalRoundPlaceholder_ignoresUnrelatedProcessor() {
    assertFalse(
        KspStepsBuilder.usesFinalRoundPlaceholder(
            ImmutableList.of(
                processorWithNames("KSP:com.example.processor.RealDummyGeneratorProvider"))));
  }

  @Test
  public void stagingSteps_removePlaceholderAfterCopiesAndBeforeZip() {
    ImmutableList<IsolatedStep> steps = stagingStepsFor(METAGEN_PROCESSOR);

    assertEquals(6, steps.size());
    assertTrue(steps.get(0) instanceof CopyIsolatedStep);
    assertTrue(steps.get(1) instanceof CopyIsolatedStep);
    assertTrue(steps.get(2) instanceof CopyIsolatedStep);
    assertTrue(steps.get(3) instanceof RmIsolatedStep);
    assertEquals(
        RelPath.get("staged/com/facebook/Dummy.java"), ((RmIsolatedStep) steps.get(3)).getPath());
    assertTrue(steps.get(4) instanceof ZipIsolatedStep);
    assertTrue(steps.get(5) instanceof CopyIsolatedStep);
  }

  @Test
  public void stagingSteps_preserveOutputsForUnrelatedProcessor() {
    ImmutableList<IsolatedStep> steps =
        stagingStepsFor("KSP:com.example.processor.RealDummyGeneratorProvider");

    assertEquals(5, steps.size());
    assertTrue(steps.get(0) instanceof CopyIsolatedStep);
    assertTrue(steps.get(1) instanceof CopyIsolatedStep);
    assertTrue(steps.get(2) instanceof CopyIsolatedStep);
    assertTrue(steps.get(3) instanceof ZipIsolatedStep);
    assertTrue(steps.get(4) instanceof CopyIsolatedStep);
  }

  private static ImmutableList<IsolatedStep> stagingStepsFor(String processorName) {
    return KspStepsBuilder.createKspOutputStagingSteps(
        mock(AbsPath.class),
        RelPath.get("kotlin"),
        RelPath.get("java"),
        RelPath.get("classes"),
        RelPath.get("staged"),
        RelPath.get("generated.src.zip"),
        RelPath.get("annotation-output"),
        ImmutableList.of(processorWithNames(processorName)));
  }

  private static ResolvedJavacPluginProperties processorWithNames(String... processorNames) {
    ResolvedJavacPluginProperties processor = mock(ResolvedJavacPluginProperties.class);
    when(processor.getProcessorNames()).thenReturn(ImmutableSortedSet.copyOf(processorNames));
    return processor;
  }
}
