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
import com.facebook.buck.jvm.cd.AbiStepsBuilder;
import com.facebook.buck.jvm.cd.BuildCommandStepsBuilder;
import com.facebook.buck.jvm.cd.DefaultCompileStepsBuilderFactory;
import com.facebook.buck.jvm.cd.LibraryStepsBuilder;
import com.facebook.buck.jvm.cd.command.BaseJarCommand;
import com.facebook.buck.jvm.cd.command.BuildMode;
import com.facebook.buck.jvm.cd.command.java.BuildJavaCommand;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.util.Objects;

/** Java steps builder */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavaStepsBuilder implements BuildCommandStepsBuilder {

  private final ImmutableList<IsolatedStep> steps;
  private final AbsPath ruleCellRoot;

  public JavaStepsBuilder(BuildJavaCommand buildJavaCommand) {
    this.ruleCellRoot = buildJavaCommand.getBaseJarCommand().getBuildCellRootPath();
    this.steps = buildSteps(buildJavaCommand);
  }

  /** Returns {@link IsolatedStep}s from the passed protobuf message */
  @Override
  public ImmutableList<IsolatedStep> getSteps() {
    return steps;
  }

  /** Returns rule cell root. */
  @Override
  public AbsPath getRuleCellRoot() {
    return ruleCellRoot;
  }

  private ImmutableList<IsolatedStep> buildSteps(BuildJavaCommand buildJavaCommand) {
    DefaultCompileStepsBuilderFactory<JavaExtraParams> factory = createDefaultStepsFactory();

    if (buildJavaCommand.getBuildMode() == BuildMode.LIBRARY) {
      LibraryStepsBuilder libraryJarBuilder = factory.getLibraryBuilder();
      handleLibraryJarCommand(libraryJarBuilder, buildJavaCommand.getBaseJarCommand());
      return libraryJarBuilder.buildIsolatedSteps();
    } else {
      AbiStepsBuilder abiJarBuilder = factory.getAbiBuilder();
      handleAbiJarCommand(abiJarBuilder, buildJavaCommand.getBaseJarCommand());
      return abiJarBuilder.buildIsolatedSteps();
    }
  }

  private void handleLibraryJarCommand(
      LibraryStepsBuilder libraryJarBuilder, BaseJarCommand baseJarCommand) {

    libraryJarBuilder.addBuildStepsForLibrary(
        baseJarCommand.getAbiCompatibilityMode(),
        baseJarCommand.getAbiGenerationMode(),
        baseJarCommand.isRequiredForSourceOnlyAbi(),
        baseJarCommand.getTrackClassUsage(),
        baseJarCommand.getBuckOut(),
        baseJarCommand.getBuildTargetValue(),
        baseJarCommand.getCompilerOutputPathsValue(),
        baseJarCommand.getCompileTimeClasspathPaths(),
        baseJarCommand.getCompileTimeClasspathSnapshotPathsMap(),
        baseJarCommand.getJavaSrcs(),
        baseJarCommand.getResourcesMap(),
        baseJarCommand.getJarParameters(),
        baseJarCommand.getBuildCellRootPath(),
        baseJarCommand.getResolvedJavac(),
        null,
        JavaExtraParams.of(baseJarCommand.getResolvedJavacOptions()));

    maybeAddMakeMissingOutputStep(baseJarCommand, libraryJarBuilder);
  }

  private void handleAbiJarCommand(AbiStepsBuilder abiJarBuilder, BaseJarCommand command) {

    abiJarBuilder.addBuildStepsForAbi(
        command.getAbiCompatibilityMode(),
        command.getAbiGenerationMode(),
        command.isRequiredForSourceOnlyAbi(),
        command.getTrackClassUsage(),
        command.getBuckOut(),
        command.getBuildTargetValue(),
        command.getCompilerOutputPathsValue(),
        command.getCompileTimeClasspathPaths(),
        command.getJavaSrcs(),
        command.getResourcesMap(),
        command.getJarParameters(),
        command.getJarParameters(),
        command.getBuildCellRootPath(),
        command.getResolvedJavac(),
        JavaExtraParams.of(command.getResolvedJavacOptions()));
  }

  private static DefaultCompileStepsBuilderFactory<JavaExtraParams> createDefaultStepsFactory() {
    DaemonJavacToJarStepFactory daemonJavacToJarStepFactory = getDaemonJavacToJarStepFactory();
    return new DefaultCompileStepsBuilderFactory<>(daemonJavacToJarStepFactory);
  }

  /** Returns {@link DaemonJavacToJarStepFactory} */
  public static DaemonJavacToJarStepFactory getDaemonJavacToJarStepFactory() {
    return new DaemonJavacToJarStepFactory();
  }

  private void maybeAddMakeMissingOutputStep(
      BaseJarCommand command, LibraryStepsBuilder javaCompileStepsBuilder) {

    javaCompileStepsBuilder.addMakeMissingOutputsStep(
        Objects.requireNonNull(command.getAnnotationPath()));
  }
}
