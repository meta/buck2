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
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.util.CapturingPrintStream;
import com.facebook.buck.util.Verbosity;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.Lists;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.List;
import java.util.Objects;
import java.util.Optional;
import java.util.stream.Collectors;
import org.jspecify.annotations.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class JavacPipelineState implements AutoCloseable {

  private static final Logger LOG = Logger.get(JavacPipelineState.class);

  private final CompilerParameters compilerParameters;
  private final ResolvedJavacOptions resolvedJavacOptions;
  private final BuildTargetValue invokingRule;
  private final ResolvedJavac resolvedJavac;
  @Nullable private final JarParameters abiJarParameters;
  @Nullable private final JarParameters libraryJarParameters;

  private final List<AutoCloseable> closeables = new ArrayList<>();

  @Nullable private CapturingPrintStream stdout;
  @Nullable private CapturingPrintStream stderr;
  private ResolvedJavac.@Nullable Invocation invocation;

  public JavacPipelineState(
      ResolvedJavac resolvedJavac,
      ResolvedJavacOptions resolvedJavacOptions,
      BuildTargetValue invokingRule,
      CompilerParameters compilerParameters,
      @Nullable JarParameters abiJarParameters,
      @Nullable JarParameters libraryJarParameters) {
    this.resolvedJavac = resolvedJavac;
    this.invokingRule = invokingRule;
    this.compilerParameters = compilerParameters;
    this.abiJarParameters = abiJarParameters;
    this.libraryJarParameters = libraryJarParameters;
    this.resolvedJavacOptions = resolvedJavacOptions;
  }

  public boolean isRunning() {
    return invocation != null;
  }

  /** Get the invocation instance. */
  public ResolvedJavac.Invocation getJavacInvocation(
      CompilerOutputPathsValue compilerOutputPathsValue,
      IsolatedExecutionContext context,
      RelPath configuredBuckOut)
      throws IOException {
    if (invocation == null) {
      stdout = new CapturingPrintStream();
      closeables.add(stdout);
      stderr = new CapturingPrintStream();
      closeables.add(stderr);
      Verbosity verbosity =
          context.getVerbosity().isSilent()
              ? Verbosity.STANDARD_INFORMATION
              : context.getVerbosity();
      IsolatedExecutionContext firstOrderContext =
          context.createSubContext(stdout, stderr, Optional.of(verbosity));
      closeables.add(firstOrderContext);

      JavacExecutionContext javacExecutionContext =
          new JavacExecutionContext(
              stderr,
              firstOrderContext.getClassLoaderCache(),
              verbosity,
              context.getRuleCellRoot(),
              configuredBuckOut);

      CompilerOutputPaths outputPaths = compilerParameters.getOutputPaths();
      invocation =
          getResolvedJavac()
              .newBuildInvocation(
                  javacExecutionContext,
                  invokingRule,
                  compilerOutputPathsValue,
                  getOptions(context, compilerParameters.getClasspathEntries()),
                  resolvedJavacOptions.getJavaAnnotationProcessorParams(),
                  resolvedJavacOptions.getStandardJavacPluginParams(),
                  compilerParameters.getSourceFilePaths(),
                  outputPaths.getPathToSourcesList(),
                  outputPaths.getWorkingDirectory(),
                  compilerParameters.getShouldTrackClassUsage(),
                  abiJarParameters,
                  libraryJarParameters,
                  compilerParameters.getAbiGenerationMode(),
                  compilerParameters.getAbiCompatibilityMode(),
                  compilerParameters.getSourceOnlyAbiRuleInfoFactory());

      closeables.add(invocation);
    }

    return invocation;
  }

  public String getStdoutContents() {
    return Objects.requireNonNull(stdout).getContentsAsString(StandardCharsets.UTF_8);
  }

  public String getStderrContents() {
    return Objects.requireNonNull(stderr).getContentsAsString(StandardCharsets.UTF_8);
  }

  @Override
  public void close() {
    for (AutoCloseable closeable : Lists.reverse(closeables)) {
      try {
        if (closeable != null) {
          closeable.close();
        }
      } catch (Exception e) {
        LOG.warn(e, "Unable to close %s; we may be leaking memory.", closeable);
      }
    }

    closeables.clear();
    stdout = null;
    stderr = null;
    invocation = null;
  }

  ResolvedJavac getResolvedJavac() {
    return resolvedJavac;
  }

  /**
   * Returns a list of command-line options to pass to javac. These options reflect the
   * configuration of this javac command.
   *
   * @return list of String command-line options.
   */
  ImmutableList<String> getOptions(
      IsolatedExecutionContext context, ImmutableList<RelPath> buildClasspathEntries) {
    CompilerOutputPaths outputPaths = compilerParameters.getOutputPaths();
    return getOptions(
        outputPaths.getClassesDir(),
        outputPaths.getAnnotationPath().getPath(),
        context,
        buildClasspathEntries);
  }

  private ImmutableList<String> getOptions(
      RelPath outputDirectory,
      Path generatedCodeDirectory,
      IsolatedExecutionContext context,
      ImmutableList<RelPath> buildClasspathEntries) {
    ImmutableList.Builder<String> builder = ImmutableList.builder();

    AbsPath ruleCellRoot = context.getRuleCellRoot();
    ResolvedJavacOptions.appendOptionsTo(
        new OptionsConsumer() {
          @Override
          public void addOptionValue(String option, String value) {
            if (option.equals("bootclasspath")) {
              builder
                  .add("-bootclasspath")
                  .add(
                      Arrays.stream(value.split(File.pathSeparator))
                          .map(path -> ruleCellRoot.resolve(path).toString())
                          .collect(Collectors.joining(File.pathSeparator)));
            } else {
              builder.add("-" + option).add(value);
            }
          }

          @Override
          public void addFlag(String flagName) {
            builder.add("-" + flagName);
          }

          @Override
          public void addExtras(Collection<String> extras) {
            builder.addAll(extras);
          }
        },
        resolvedJavacOptions,
        ruleCellRoot);
    Optional<String> bootclasspath =
        ResolvedJavacOptions.Companion.getBootclasspathString(
            resolvedJavacOptions.getBootclasspathList());

    // verbose flag, if appropriate.
    if (context.getVerbosity().shouldUseVerbosityFlagIfAvailable()) {
      builder.add("-verbose");
    }

    // Specify the output directory.
    builder.add("-d").add(ruleCellRoot.resolve(outputDirectory).toString());

    if (resolvedJavacOptions.isJavaAnnotationProcessorParamsPresent()) {
      builder.add("-s").add(ruleCellRoot.resolve(generatedCodeDirectory).toString());
    }

    List<String> classpathEntries = new ArrayList<>();
    int targetRelease = resolvedJavacOptions.getLanguageLevelOptions().getTargetLevelValue();
    if (targetRelease > 8) {
      String systemImage = resolvedJavacOptions.getSystemImage();
      if (systemImage != null) {
        classpathEntries.add(bootclasspath.get());

        builder.add("--system");
        builder.add(systemImage);
      }
    }
    // else, bootclasspath is already handled by the OptionsConsumer above

    classpathEntries.addAll(
        buildClasspathEntries.stream()
            .map(ruleCellRoot::resolve)
            .map(AbsPath::normalize)
            .map(AbsPath::toString)
            .toList());

    // Build up and set the classpath.
    if (!classpathEntries.isEmpty()) {
      String classpath = Joiner.on(File.pathSeparator).join(classpathEntries);
      builder.add("-classpath", classpath);
    } else {
      builder.add("-classpath", "''");
    }

    return builder.build();
  }

  public CompilerParameters getCompilerParameters() {
    return compilerParameters;
  }

  Optional<JarParameters> getLibraryJarParameters() {
    return Optional.ofNullable(libraryJarParameters);
  }

  Optional<JarParameters> getAbiJarParameters() {
    return Optional.ofNullable(abiJarParameters);
  }
}
