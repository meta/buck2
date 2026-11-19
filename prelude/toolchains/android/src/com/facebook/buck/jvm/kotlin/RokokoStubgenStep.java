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

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.StepExecutionResults;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

/**
 * Rokoko stubgen step (prototype). Runs stubgen in-worker through JNI. Selected when the {@code
 * rokoko_stubgen_so} Kosabi plugin option is present (i.e. {@code kotlin.stubgen_engine=rokoko}).
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class RokokoStubgenStep implements IsolatedStep {
  private static final Logger LOG = Logger.get(RokokoStubgenStep.class);

  private final BuildTargetValue invokingRule;
  private final AbsPath rokokoStubgenSo;
  private final ImmutableSortedSet<RelPath> sourceFilePaths;
  private final ImmutableList<AbsPath> compilationClasspath;
  private final RelPath stubgenDir;
  private final RelPath stubClassOutputDir;
  private final RelPath reportsOutput;

  RokokoStubgenStep(
      BuildTargetValue invokingRule,
      AbsPath rokokoStubgenSo,
      ImmutableSortedSet<RelPath> sourceFilePaths,
      ImmutableList<AbsPath> compilationClasspath,
      RelPath stubgenDir,
      RelPath stubClassOutputDir,
      RelPath reportsOutput) {
    this.invokingRule = invokingRule;
    this.rokokoStubgenSo = rokokoStubgenSo;
    this.sourceFilePaths = sourceFilePaths;
    this.compilationClasspath = compilationClasspath;
    this.stubgenDir = stubgenDir;
    this.stubClassOutputDir = stubClassOutputDir;
    this.reportsOutput = reportsOutput;
  }

  @Override
  public String getShortName() {
    return RokokoStubgenStep.class.getSimpleName();
  }

  @Override
  public String getIsolatedStepDescription(IsolatedExecutionContext context) {
    return String.format(
        "rokoko stubgen (%s, %d sources)",
        invokingRule.getFullyQualifiedName(), sourceFilePaths.size());
  }

  @Override
  public StepExecutionResult executeIsolatedStep(IsolatedExecutionContext context) {
    AbsPath ruleCellRoot = context.getRuleCellRoot();

    List<String> sources = new ArrayList<>();
    List<String> unsupported = new ArrayList<>();
    for (RelPath src : sourceFilePaths) {
      if (src.getPath().toString().endsWith(".kt")) {
        sources.add(ruleCellRoot.resolve(src).getPath().toString());
      } else {
        unsupported.add(src.toString());
      }
    }
    if (!unsupported.isEmpty()) {
      return new StepExecutionResult(
          1,
          Optional.of(
              "rokoko stubgen prototype supports .kt sources only, found: "
                  + Joiner.on(", ").join(unsupported)));
    }
    if (sources.isEmpty()) {
      return new StepExecutionResult(1, Optional.of("rokoko stubgen: no .kt sources"));
    }
    if (rokokoStubgenSo == null) {
      return new StepExecutionResult(
          1,
          Optional.of("kotlin.rokoko_stubgen_so is not set: rokoko stubgen runs in-worker only"));
    }

    ImmutableList<String> classpathStrings =
        compilationClasspath.stream()
            .map(path -> path.getPath().toString())
            .collect(ImmutableList.toImmutableList());
    String classesOut = ruleCellRoot.resolve(stubClassOutputDir).getPath().toString();
    String stubsOut = ruleCellRoot.resolve(stubgenDir).getPath().toString();
    Path reportsDir = ruleCellRoot.resolve(reportsOutput).getPath();
    String logPath = reportsDir.resolve("rokoko_stubgen.log").toString();

    try {
      Files.createDirectories(reportsDir);
      Files.createDirectories(ruleCellRoot.resolve(stubClassOutputDir).getPath());
      Files.createDirectories(ruleCellRoot.resolve(stubgenDir).getPath());
      // Repeat loads of the same library are JVM no-ops; the first load wins.
      System.load(rokokoStubgenSo.getPath().toString());
      String error =
          RokokoStubgenNative.generate(
              sources.toArray(new String[0]),
              classpathStrings.toArray(new String[0]),
              classesOut,
              stubsOut,
              logPath);
      LOG.info(
          "ROKOKO_STUBGEN|%s|jni|sources=%d|error=%s",
          invokingRule.getFullyQualifiedName(), sources.size(), error);
      if (error != null) {
        return new StepExecutionResult(1, Optional.of(error));
      }
      return StepExecutionResults.SUCCESS;
    } catch (IOException | UnsatisfiedLinkError | RuntimeException e) {
      return new StepExecutionResult(1, Optional.of("rokoko stubgen failed: " + e));
    }
  }
}
