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

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.StepExecutionResults;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Joiner;
import com.google.common.base.MoreObjects;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import java.io.IOException;
import java.util.Optional;
import javax.annotation.Nullable;

/** Command used to compile java libraries with a variety of ways to handle dependencies. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavacStep implements IsolatedStep {

  private final JavacPipelineState state;
  private final BuildTargetValue invokingRule;
  private final RelPath configuredBuckOut;
  private final boolean ownsPipelineObject;
  private final CompilerOutputPathsValue compilerOutputPathsValue;
  private final boolean skipIfNoCompilationUnits;

  private final boolean mixedModule;

  public JavacStep(
      ResolvedJavac resolvedJavac,
      ResolvedJavacOptions javacOptions,
      BuildTargetValue invokingRule,
      RelPath configuredBuckOut,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters compilerParameters,
      boolean skipIfNoCompilationUnits,
      @Nullable JarParameters abiJarParameters,
      @Nullable JarParameters libraryJarParameters,
      boolean mixedCompilation) {
    this(
        new JavacPipelineState(
            resolvedJavac,
            javacOptions,
            invokingRule,
            compilerParameters,
            abiJarParameters,
            libraryJarParameters),
        invokingRule,
        configuredBuckOut,
        true,
        compilerOutputPathsValue,
        skipIfNoCompilationUnits,
        mixedCompilation);
  }

  public JavacStep(
      JavacPipelineState state,
      BuildTargetValue invokingRule,
      RelPath configuredBuckOut,
      CompilerOutputPathsValue compilerOutputPathsValue) {
    this(state, invokingRule, configuredBuckOut, false, compilerOutputPathsValue, false, false);
  }

  private JavacStep(
      JavacPipelineState state,
      BuildTargetValue invokingRule,
      RelPath configuredBuckOut,
      boolean ownsPipelineObject,
      CompilerOutputPathsValue compilerOutputPathsValue,
      boolean skipIfNoCompilationUnits,
      boolean mixedModule) {
    this.state = state;
    this.invokingRule = invokingRule;
    this.configuredBuckOut = configuredBuckOut;
    this.ownsPipelineObject = ownsPipelineObject;
    this.compilerOutputPathsValue = compilerOutputPathsValue;
    this.skipIfNoCompilationUnits = skipIfNoCompilationUnits;
    this.mixedModule = mixedModule;
  }

  @Override
  public final StepExecutionResult executeIsolatedStep(IsolatedExecutionContext context)
      throws IOException, InterruptedException {

    if (skipIfNoCompilationUnits && !hasCompilationUnits(context.getRuleCellRoot())) {
      return StepExecutionResults.SUCCESS;
    }

    int exitCode;
    Optional<String> stderr = Optional.empty();
    try {
      ResolvedJavac.Invocation invocation =
          state.getJavacInvocation(compilerOutputPathsValue, context, configuredBuckOut);

      if (invokingRule.isSourceAbi()) {
        exitCode = invocation.buildSourceAbiJar();
      } else if (invokingRule.isSourceOnlyAbi()) {
        exitCode = invocation.buildSourceOnlyAbiJar(mixedModule);
      } else {
        exitCode = invocation.buildClasses();
      }

      if (exitCode != StepExecutionResults.SUCCESS_EXIT_CODE) {
        stderr = Optional.of(state.getStderrContents());
      }

    } finally {
      if (ownsPipelineObject) {
        state.close();
      }
    }

    return new StepExecutionResult(exitCode, stderr);
  }

  /**
   * KSP and KAPT stage a {@code .src.zip} into the Java source list before their processors have
   * run, so whether javac has anything to compile is not known until this step executes.
   */
  private boolean hasCompilationUnits(AbsPath ruleCellRoot) throws IOException {
    return JavaPaths.hasJavaCompilationUnits(
        state.getCompilerParameters().getSourceFilePaths().stream()
            .map(path -> ruleCellRoot.resolve(path).getPath())
            .collect(ImmutableList.toImmutableList()));
  }

  @VisibleForTesting
  ResolvedJavac getResolvedJavac() {
    return state.getResolvedJavac();
  }

  @Override
  public String getIsolatedStepDescription(IsolatedExecutionContext context) {
    String description =
        getResolvedJavac()
            .getDescription(
                getOptions(context, getClasspathEntries()),
                state.getCompilerParameters().getOutputPaths().getPathToSourcesList());

    if (invokingRule.isLibraryJar() && state.getLibraryJarParameters().isPresent()) {
      JarParameters jarParameters = state.getLibraryJarParameters().get();
      Optional<RelPath> manifestFile = jarParameters.getManifestFile();
      ImmutableSortedSet<RelPath> entriesToJar = jarParameters.getEntriesToJar();
      description =
          description
              + "; "
              + String.format(
                  "jar %s %s %s %s",
                  manifestFile.map(ignore -> "cfm").orElse("cf"),
                  jarParameters.getJarPath(),
                  manifestFile.map(RelPath::toString).orElse(""),
                  Joiner.on(' ').join(entriesToJar));
    }

    return description;
  }

  @Override
  public String getShortName() {
    String name;
    if (invokingRule.isSourceAbi()) {
      return "source_abi";
    } else if (invokingRule.isSourceOnlyAbi()) {
      return "source_only_abi";
    } else if (state.getLibraryJarParameters().isPresent()) {
      name = "javac";
    } else {
      name = getResolvedJavac().getShortName();
    }

    return name;
  }

  /**
   * Returns a list of command-line options to pass to javac. These options reflect the
   * configuration of this javac command.
   *
   * @param context the ExecutionContext with in which javac will run
   * @return list of String command-line options.
   */
  @VisibleForTesting
  ImmutableList<String> getOptions(
      IsolatedExecutionContext context, ImmutableList<RelPath> buildClasspathEntries) {
    return state.getOptions(context, buildClasspathEntries);
  }

  /**
   * @return The classpath entries used to invoke javac.
   */
  @VisibleForTesting
  ImmutableList<RelPath> getClasspathEntries() {
    return state.getCompilerParameters().getClasspathEntries();
  }

  @VisibleForTesting
  ImmutableSortedSet<RelPath> getSrcs() {
    return state.getCompilerParameters().getSourceFilePaths();
  }

  @Override
  public String toString() {
    return MoreObjects.toStringHelper(this)
        .add("invokingRule", invokingRule)
        .add("ownsPipelineObject", ownsPipelineObject)
        .toString();
  }
}
