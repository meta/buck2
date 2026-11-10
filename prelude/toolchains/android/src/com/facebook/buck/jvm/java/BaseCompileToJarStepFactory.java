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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MakeCleanDirectoryIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MkdirIsolatedStep;
import com.facebook.buck.step.isolatedsteps.java.JarDirectoryStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableList.Builder;
import com.google.common.collect.ImmutableMap;
import java.util.Objects;
import java.util.Optional;
import org.jspecify.annotations.Nullable;

/** Provides a base implementation for post compile steps. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public abstract class BaseCompileToJarStepFactory<T extends CompileToJarStepFactory.ExtraParams>
    implements CompileToJarStepFactory<T> {

  protected BaseCompileToJarStepFactory() {}

  @Override
  public final void createCompileToJarStep(
      RelPath buckOut,
      AbsPath buildCellRootPath,
      BuildTargetValue buildTargetValue,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters compilerParameters,
      @Nullable JarParameters abiJarParameters,
      @Nullable JarParameters libraryJarParameters,
      Builder<IsolatedStep> steps,
      ImmutableMap<RelPath, RelPath> resourcesMap,
      ResolvedJavac resolvedJavac,
      @Nullable ActionMetadata actionMetadata,
      T extraParams) {
    Preconditions.checkArgument(libraryJarParameters != null || abiJarParameters == null);

    steps.addAll(
        getCompilerSetupIsolatedSteps(
            resourcesMap,
            compilerParameters.getOutputPaths(),
            compilerParameters.getSourceFilePaths().isEmpty()));

    JarParameters jarParameters =
        abiJarParameters != null ? abiJarParameters : libraryJarParameters;
    if (jarParameters != null) {
      addJarSetupSteps(jarParameters, steps);
    }

    // Only run javac if there are .java or .kt files to compile or we need to shovel the manifest
    // file
    // into the built jar.
    if (!compilerParameters.getSourceFilePaths().isEmpty()) {
      // This adds the javac command, along with any supporting commands.
      createCompileToJarStepImpl(
          buckOut,
          buildCellRootPath,
          buildTargetValue,
          compilerOutputPathsValue,
          compilerParameters,
          abiJarParameters,
          libraryJarParameters,
          steps,
          resolvedJavac,
          actionMetadata,
          extraParams);
    }

    if (jarParameters != null) {
      addJarCreationSteps(compilerParameters, steps, jarParameters);
    }
  }

  /** Returns Compiler Setup steps */
  public ImmutableList<IsolatedStep> getCompilerSetupIsolatedSteps(
      ImmutableMap<RelPath, RelPath> resourcesMap,
      CompilerOutputPaths outputPaths,
      boolean emptySources) {
    // Always create the output directory, even if there are no .java files to compile because there
    // might be resources that need to be copied there.

    Builder<IsolatedStep> steps = ImmutableList.builder();

    steps.addAll(MakeCleanDirectoryIsolatedStep.of(outputPaths.getClassesDir()));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(outputPaths.getAnnotationPath()));

    // If there are resources, then link them to the appropriate place in the classes directory.
    steps.addAll(CopyResourcesStep.of(resourcesMap));

    if (!emptySources) {
      steps.add(
          new MkdirIsolatedStep(
              Objects.requireNonNull(outputPaths.getPathToSourcesList().getParent())));
      steps.add(new MkdirIsolatedStep(outputPaths.getWorkingDirectory()));
    }

    return steps.build();
  }

  protected void addJarSetupSteps(JarParameters jarParameters, Builder<IsolatedStep> steps) {
    steps.add(
        new MkdirIsolatedStep(Objects.requireNonNull(jarParameters.getJarPath().getParent())));
  }

  void addJarCreationSteps(
      CompilerParameters compilerParameters,
      Builder<IsolatedStep> steps,
      JarParameters jarParameters) {
    // No source files, only resources
    if (compilerParameters.getSourceFilePaths().isEmpty()) {
      steps.add(new JarDirectoryStep(jarParameters));
    }
  }

  protected void createCompileToJarStepImpl(
      RelPath buckOut,
      AbsPath buildCellRootPath,
      BuildTargetValue target,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters compilerParameters,
      @Nullable JarParameters abiJarParameters,
      @Nullable JarParameters libraryJarParameters,
      Builder<IsolatedStep> steps,
      ResolvedJavac resolvedJavac,
      @Nullable ActionMetadata actionMetadata,
      T extraParams) {
    Preconditions.checkArgument(abiJarParameters == null);
    Preconditions.checkArgument(
        libraryJarParameters != null
            && libraryJarParameters
                .getEntriesToJar()
                .contains(compilerParameters.getOutputPaths().getClassesDir()));

    createCompileStep(
        buckOut,
        buildCellRootPath,
        target,
        compilerOutputPathsValue,
        compilerParameters,
        steps,
        resolvedJavac,
        actionMetadata,
        extraParams,
        null,
        false);

    steps.add(new JarDirectoryStep(libraryJarParameters));
  }

  /**
   * This can be used make the bootclasspath if available, to the postprocess classes commands.
   *
   * @return the bootclasspath.
   */
  protected Optional<String> getBootClasspath() {
    return Optional.empty();
  }

  public abstract void createCompileStep(
      RelPath buckOut,
      AbsPath buildCellRootPath,
      BuildTargetValue invokingRule,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters parameters,
      Builder<IsolatedStep> steps,
      ResolvedJavac resolvedJavac,
      @Nullable ActionMetadata actionMetadata,
      T extraParams,
      @Nullable JarParameters abiJarParameters,
      boolean mixedCompilationMode);

  public boolean supportsCompilationDaemon() {
    return false;
  }
}
