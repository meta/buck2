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

import static org.hamcrest.MatcherAssert.assertThat;
import static org.hamcrest.Matchers.equalTo;
import static org.hamcrest.Matchers.hasItem;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.cd.model.java.AbiGenerationMode;
import com.facebook.buck.cd.model.java.BuildTargetValue.Type;
import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.StepExecutionResults;
import com.facebook.buck.step.TestExecutionContext;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.base.Splitter;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableSortedSet;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.List;
import java.util.Optional;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;
import org.hamcrest.Matchers;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.ExpectedException;

public class JavacStepTest {

  @Rule public ExpectedException thrown = ExpectedException.none();
  @Rule public TemporaryPaths tmp = new TemporaryPaths();

  private String target;
  private BuildTargetValue buildTargetValue;
  private RelPath configuredBuckOut;
  private CompilerParameters compilerParameters;

  @Before
  public void setUp() {
    target = "//foo:bar";
    buildTargetValue = new BuildTargetValue(Type.LIBRARY, target);
    configuredBuckOut = RelPath.get("buck-out/v2");
    compilerParameters =
        new CompilerParameters(
            ImmutableSortedSet.of(),
            ImmutableList.of(),
            ImmutableList.of(),
            getCompilerOutputPaths(),
            AbiGenerationMode.CLASS,
            AbiGenerationMode.CLASS,
            false,
            null);
  }

  @Test
  public void successfulCompileDoesNotSendStdoutAndStderrToConsole() throws Exception {
    ResolvedJavacOptions javacOptions = getResolvedJavacOptions();

    JavacStep step =
        new JavacStep(
            new FakeJavac(0, "javac stderr\n"),
            javacOptions,
            buildTargetValue,
            configuredBuckOut,
            getCompilerOutputPathsValue(),
            compilerParameters,
            /* skipIfNoCompilationUnits */ false,
            null,
            null,
            false);

    AbsPath rootPath = tmp.getRoot();
    IsolatedExecutionContext executionContext = TestExecutionContext.newInstance(rootPath);
    StepExecutionResult result = step.executeIsolatedStep(executionContext);

    // Note that we don't include stderr in the step result on success.
    assertThat(result, equalTo(StepExecutionResults.SUCCESS));
  }

  @Test
  public void failedCompileSendsStdoutAndStderrToConsole() throws Exception {
    ResolvedJavacOptions javacOptions = getResolvedJavacOptions();

    JavacStep step =
        new JavacStep(
            new FakeJavac(3, "javac stderr\n"),
            javacOptions,
            buildTargetValue,
            configuredBuckOut,
            getCompilerOutputPathsValue(),
            compilerParameters,
            /* skipIfNoCompilationUnits */ false,
            null,
            null,
            false);

    AbsPath rootPath = tmp.getRoot();
    IsolatedExecutionContext executionContext = TestExecutionContext.newInstance(rootPath);
    StepExecutionResult result = step.executeIsolatedStep(executionContext);

    // JavacStep itself writes stdout to the console on error; we expect the Build class to write
    // the stderr stream returned in the StepExecutionResult
    assertThat(
        result,
        equalTo(
            new StepExecutionResult(
                StepExecutionResults.ERROR_EXIT_CODE, Optional.of("javac stderr\n"))));
  }

  @Test
  public void existingBootclasspathDirSucceeds() throws Exception {
    ResolvedJavacOptions javacOptions =
        getResolvedJavacOptions(ImmutableList.of(RelPath.get("this-totally-exists")));

    JavacStep step =
        new JavacStep(
            new FakeJavac(0, "javac stderr\n"),
            javacOptions,
            buildTargetValue,
            configuredBuckOut,
            getCompilerOutputPathsValue(),
            compilerParameters,
            /* skipIfNoCompilationUnits */ false,
            null,
            null,
            false);

    AbsPath rootPath = tmp.getRoot();
    IsolatedExecutionContext executionContext = TestExecutionContext.newInstance(rootPath);
    StepExecutionResult result = step.executeIsolatedStep(executionContext);

    assertThat(result, equalTo(StepExecutionResults.SUCCESS));
  }

  @Test
  public void bootclasspathResolvedToAbsolutePath() {
    ResolvedJavacOptions javacOptions =
        getResolvedJavacOptions(ImmutableList.of(RelPath.get("this-totally-exists:relative-path")));

    JavacStep step =
        new JavacStep(
            new FakeJavac(0, "javac stderr\n"),
            javacOptions,
            buildTargetValue,
            configuredBuckOut,
            getCompilerOutputPathsValue(),
            compilerParameters,
            /* skipIfNoCompilationUnits */ false,
            null,
            null,
            false);

    AbsPath rootPath = tmp.getRoot();
    IsolatedExecutionContext executionContext = TestExecutionContext.newInstance(rootPath);

    String description = step.getIsolatedStepDescription(executionContext);
    List<String> options =
        Splitter.on(",")
            .trimResults()
            .splitToList(Splitter.on("Delimiter").splitToList(description).get(0));
    assertThat(options, hasItem("-bootclasspath"));
    int bootclasspathIndex = options.indexOf("-bootclasspath");
    String bootclasspath = options.get(bootclasspathIndex + 1);
    assertThat(bootclasspath, Matchers.not(Matchers.emptyOrNullString()));
    for (String path : Splitter.on(File.pathSeparator).split(bootclasspath)) {
      assertTrue(Paths.get(path).isAbsolute());
    }
  }

  @Test
  public void sourceZipWithoutJavaEntriesSkipsCompilation() throws Exception {
    JavacStep step = skippableJavacStep(writeSourceZip("com/facebook/resource.txt"));

    StepExecutionResult result =
        step.executeIsolatedStep(TestExecutionContext.newInstance(tmp.getRoot()));

    // FakeJavac would have reported exit code 3 had javac been invoked.
    assertThat(result, equalTo(StepExecutionResults.SUCCESS));
  }

  @Test
  public void sourceZipWithJavaEntriesRunsCompilation() throws Exception {
    JavacStep step = skippableJavacStep(writeSourceZip("com/facebook/Dummy.java"));

    StepExecutionResult result =
        step.executeIsolatedStep(TestExecutionContext.newInstance(tmp.getRoot()));

    assertThat(
        result,
        equalTo(
            new StepExecutionResult(
                StepExecutionResults.ERROR_EXIT_CODE, Optional.of("javac stderr\n"))));
  }

  private RelPath writeSourceZip(String... entryNames) throws IOException {
    RelPath zipPath = RelPath.get("generated.src.zip");
    try (ZipOutputStream out =
        new ZipOutputStream(Files.newOutputStream(tmp.getRoot().resolve(zipPath).getPath()))) {
      for (String entryName : entryNames) {
        out.putNextEntry(new ZipEntry(entryName));
        out.closeEntry();
      }
    }
    return zipPath;
  }

  private JavacStep skippableJavacStep(RelPath sourceFilePath) {
    CompilerParameters parameters =
        new CompilerParameters(
            ImmutableSortedSet.orderedBy(RelPath.comparator()).add(sourceFilePath).build(),
            ImmutableList.of(),
            ImmutableList.of(),
            getCompilerOutputPaths(),
            AbiGenerationMode.CLASS,
            AbiGenerationMode.CLASS,
            false,
            null);

    return new JavacStep(
        new FakeJavac(3, "javac stderr\n"),
        getResolvedJavacOptions(),
        buildTargetValue,
        configuredBuckOut,
        getCompilerOutputPathsValue(),
        parameters,
        /* skipIfNoCompilationUnits */ true,
        null,
        null,
        false);
  }

  private static ResolvedJavacOptions getResolvedJavacOptions() {
    return getResolvedJavacOptions(ImmutableList.of());
  }

  private static ResolvedJavacOptions getResolvedJavacOptions(
      ImmutableList<RelPath> bootclasspathList) {
    return new ResolvedJavacOptions(
        bootclasspathList,
        JavacLanguageLevelOptions.DEFAULT,
        false /* debug */,
        false /* verbose */,
        JavacPluginParams.EMPTY /* javaAnnotationProcessorParams */,
        JavacPluginParams.EMPTY /* standardJavacPluginParams */,
        ImmutableList.of() /* extraArguments */,
        null /* systemImage */);
  }

  private CompilerOutputPathsValue getCompilerOutputPathsValue() {
    return CompilerOutputPathsValue.of(
        target, getCompilerOutputPaths(), getCompilerOutputPaths(), getCompilerOutputPaths());
  }

  private static CompilerOutputPaths getCompilerOutputPaths() {
    return new CompilerOutputPaths(
        RelPath.get("classesDir"),
        RelPath.get("outputJarDirPath"),
        Optional.empty(),
        RelPath.get("annotationPath"),
        RelPath.get("pathToSourcesList"),
        RelPath.get("workingDirectory"),
        Optional.empty());
  }
}
