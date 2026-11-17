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

import static com.facebook.buck.jvm.java.CompilerOutputPaths.getKotlinTempDepFilePath;
import static com.google.common.collect.Iterables.transform;

import com.facebook.buck.core.build.execution.context.IsolatedExecutionContext;
import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.java.CompilerClassToSourceMap;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext;
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason;
import com.facebook.buck.jvm.kotlin.plugin.PluginLoader;
import com.facebook.buck.step.StepExecutionResult;
import com.facebook.buck.step.StepExecutionResults;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.util.CapturingPrintStream;
import com.facebook.buck.util.Verbosity;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import com.google.common.base.Joiner;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;
import java.io.File;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.Objects;
import java.util.Optional;
import java.util.function.Supplier;
import org.jspecify.annotations.Nullable;

/** Kotlin compile Step */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlincStep implements IsolatedStep {
  private static final Logger LOG = Logger.get(KotlincStep.class);

  private static final String CLASSPATH_FLAG = "-classpath";
  private static final String DESTINATION_FLAG = "-d";
  // Under the incremental state directory, beside the directories it copies.
  private static final String STATE_COPY_DIRECTORY = "applicability_full_round_copy";
  private static final String X_PLUGIN_ARG = "-Xplugin=";
  private static final String PLUGIN = "-P";
  // The marker the applicability plugin's report starts with (`ApplicabilityReport.kt`).
  private static final String APPLICABILITY_REPORT_MARKER = "KOSABI_APPLICABILITY_REPORT_V2=";
  // Written by the incremental compiler after a successful round
  // (`JvmCompilationConfigurationFactory`).
  private static final String SHRUNK_CLASSPATH_SNAPSHOT = "shrunk-classpath-snapshot.bin";
  private static final String APPLICABILITY_PLUGIN_ID =
      "com.facebook.kotlin.compilerplugins.kosabiapplicability";

  private static final int EXPECTED_SOURCE_ONLY_ABI_EXIT_CODE = 2;

  private final Kotlinc kotlinc;
  private final ImmutableList<AbsPath> combinedClassPathEntries;
  private final ImmutableList<AbsPath> kotlinHomeLibraries;
  private final Path outputDirectory;
  private final ImmutableList<String> extraArguments;
  private final ImmutableList<String> verboseModeOnlyExtraArguments;
  private final ImmutableSortedSet<RelPath> sourceFilePaths;
  private final Path pathToSrcsList;
  private final RelPath reportDirPath;
  private final BuildTargetValue invokingRule;
  private final CompilerOutputPaths outputPaths;
  private final boolean trackClassUsage;
  private final RelPath configuredBuckOut;
  private final ImmutableMap<String, AbsPath> resolvedKosabiPluginOptionPath;
  private final @Nullable String kosabiEarlyTerminationMessagePrefix;
  // For SO-ABI builds: reduced classpath (rfsoa deps only), used as kotlinc -classpath.
  // For library builds: full dep set, used only by KosabiStubgen/KSP — NOT for applicability.
  private final ImmutableList<AbsPath> compilationClasspath;
  // Reduced SO-ABI classpath for the applicability plugin (rfsoa + source_only_abi_deps only).
  private final ImmutableList<AbsPath> applicabilityClasspath;
  private final boolean verifySourceOnlyAbiConstraints;
  private ImmutableList<IsolatedStep> postKotlinCompilationFailureSteps;
  private final Optional<AbsPath> depTrackerPath;
  private final KotlincMode kotlincMode;
  private final KotlinCDAnalytics kotlinCDAnalytics;
  private final LanguageVersion languageVersion;
  private final boolean shouldKosabiJvmAbiGenUseK2;
  private final boolean shouldKosabiUseShallowClassUsage;
  private final String kosabiAbiValidationMode;
  private final ImmutableList<String> kosabiJvmAbiGenOptions;
  private final boolean disableKosabiKeptBodyStubCheck;

  private static final String COMPOSE_PLUGIN_PATH_FRAGMENT = "compose-compiler-plugin";

  KotlincStep(
      BuildTargetValue invokingRule,
      Path outputDirectory,
      ImmutableSortedSet<RelPath> sourceFilePaths,
      Path pathToSrcsList,
      ImmutableList<AbsPath> combinedClassPathEntries,
      ImmutableList<AbsPath> kotlinHomeLibraries,
      RelPath reportDirPath,
      Kotlinc kotlinc,
      ImmutableList<String> extraArguments,
      ImmutableList<String> verboseModeOnlyExtraArguments,
      CompilerOutputPaths outputPaths,
      boolean trackClassUsage,
      RelPath configuredBuckOut,
      ImmutableMap<String, AbsPath> resolvedKosabiPluginOptionPath,
      @Nullable String kosabiEarlyTerminationMessagePrefix,
      ImmutableList<AbsPath> compilationClasspath,
      ImmutableList<AbsPath> applicabilityClasspath,
      boolean verifySourceOnlyAbiConstraints,
      ImmutableList<IsolatedStep> postKotlinCompilationFailureSteps,
      Optional<AbsPath> depTrackerPath,
      KotlincMode kotlincMode,
      KotlinCDAnalytics kotlinCDAnalytics,
      LanguageVersion languageVersion,
      boolean shouldKosabiJvmAbiGenUseK2,
      boolean shouldKosabiUseShallowClassUsage,
      String kosabiAbiValidationMode,
      ImmutableList<String> kosabiJvmAbiGenOptions,
      boolean disableKosabiKeptBodyStubCheck) {
    this.invokingRule = invokingRule;
    this.outputDirectory = outputDirectory;
    this.sourceFilePaths = sourceFilePaths;
    this.pathToSrcsList = pathToSrcsList;
    this.reportDirPath = reportDirPath;
    this.kotlinc = kotlinc;
    this.combinedClassPathEntries = combinedClassPathEntries;
    this.kotlinHomeLibraries = kotlinHomeLibraries;
    this.extraArguments = extraArguments;
    this.verboseModeOnlyExtraArguments = verboseModeOnlyExtraArguments;
    this.outputPaths = outputPaths;
    this.trackClassUsage = trackClassUsage;
    this.configuredBuckOut = configuredBuckOut;
    this.resolvedKosabiPluginOptionPath = resolvedKosabiPluginOptionPath;
    this.kosabiEarlyTerminationMessagePrefix = kosabiEarlyTerminationMessagePrefix;
    this.compilationClasspath = compilationClasspath;
    this.applicabilityClasspath = applicabilityClasspath;
    this.verifySourceOnlyAbiConstraints = verifySourceOnlyAbiConstraints;
    this.postKotlinCompilationFailureSteps = postKotlinCompilationFailureSteps;
    this.depTrackerPath = depTrackerPath;
    this.kotlincMode = kotlincMode;
    this.kotlinCDAnalytics = kotlinCDAnalytics;
    this.languageVersion = languageVersion;
    this.shouldKosabiJvmAbiGenUseK2 = shouldKosabiJvmAbiGenUseK2;
    this.shouldKosabiUseShallowClassUsage = shouldKosabiUseShallowClassUsage;
    this.kosabiAbiValidationMode = kosabiAbiValidationMode;
    this.kosabiJvmAbiGenOptions = kosabiJvmAbiGenOptions;
    this.disableKosabiKeptBodyStubCheck = disableKosabiKeptBodyStubCheck;
  }

  @Override
  public String getShortName() {
    return getKotlinc().getShortName();
  }

  protected boolean emitsClassToSourceMap() {
    return true;
  }

  @Override
  public StepExecutionResult executeIsolatedStep(IsolatedExecutionContext context) {
    ImmutableList<String> compilerOptions = getOptions(context, combinedClassPathEntries);
    Verbosity verbosity =
        context.getVerbosity().isSilent() ? Verbosity.STANDARD_INFORMATION : context.getVerbosity();
    try (CapturingPrintStream stdout = new CapturingPrintStream();
        CapturingPrintStream stderr = new CapturingPrintStream();
        IsolatedExecutionContext firstOrderContext =
            context.createSubContext(stdout, stderr, Optional.of(verbosity))) {

      KotlinCDLoggingContext loggingContext =
          KotlinCDLoggingContextFactory.create(this, languageVersion, kotlincMode);
      Instant compilationStart = Instant.now();

      @Nullable Path roundStateDir = applicabilityRoundStateDir();
      int declaredDepsBuildResult =
          compileCheckingAgainInFull(
              verifySourceOnlyAbiConstraints && invokingRule.isLibraryJar(),
              kotlincMode,
              mode ->
                  compile(
                      context,
                      firstOrderContext,
                      compilerOptions,
                      mode,
                      loggingContext,
                      roundStateDir),
              kotlinc::lastCompilationWasIncremental,
              () -> stderr.getContentsAsString(StandardCharsets.UTF_8),
              () -> {
                stdout.reset();
                stderr.reset();
              },
              () -> copyIncrementalState(context.getRuleCellRoot()),
              invokingRule.getFullyQualifiedName());

      Instant compilationEnd = Instant.now();
      Duration compilationDuration = Duration.between(compilationStart, compilationEnd);
      LOG.info(
          "KOTLINCD_STEP_DURATION|%s|%s|%d|%d",
          invokingRule.getFullyQualifiedName(),
          this.getClass().getSimpleName(),
          compilationDuration.toMillis(),
          sourceFilePaths.size());
      loggingContext.addExtras(
          this.getClass().getSimpleName(),
          "Kotlinc step duration: " + compilationDuration.toMillis() + " ms");
      loggingContext.setDurationMs(compilationDuration.toMillis());
      kotlinCDAnalytics.log(loggingContext);

      String firstOrderStderr = stderr.getContentsAsString(StandardCharsets.UTF_8);
      Optional<String> returnedStderr;

      // We're generating Kotlin source-only-abi with Kosabi, a set of Kotlin compiler
      // plugins.
      // see `Kosabi.java`
      //
      // `jvm-abi-gen` is one of Kosabi plugins responsible for ABI class files
      // generation.
      // `jvm-abi-gen` could pass only the Kotlin Frontend Compiler stage, thus it's
      // intentionally
      // throws an Internal Compiler Error and exits with the corresponding exit code.
      //
      // EXPECTED_SOURCE_ONLY_ABI_EXIT_CODE is Kotlin compiler Internal Error code.
      // We should treat Internal Compiler Error in source-only-abi as an abi-generation
      // Success.
      // In addition to an exit code we're expecting a specific `firstOrderStderr` message
      // We both need an exit code and a message to prevent cache poisoning in cases when
      // Kotlin compiler had a legit Internal Error
      boolean shouldCheckSourceOnlyAbiErrorMessage = kosabiEarlyTerminationMessagePrefix != null;

      boolean isExpectedSourceOnlyAbiTermination =
          !shouldCheckSourceOnlyAbiErrorMessage
              || validateEarlyTerminationErrorMessage(context, firstOrderStderr);

      if (invokingRule.isSourceOnlyAbi()
          && declaredDepsBuildResult == EXPECTED_SOURCE_ONLY_ABI_EXIT_CODE
          && isExpectedSourceOnlyAbiTermination) {
        declaredDepsBuildResult = StepExecutionResults.SUCCESS_EXIT_CODE;
        returnedStderr = Optional.empty();
      } else if (declaredDepsBuildResult != StepExecutionResults.SUCCESS_EXIT_CODE) {
        for (IsolatedStep step : postKotlinCompilationFailureSteps) {
          LOG.debug("Executing step [%s] after KotlinC failure", step.getShortName());
          step.executeIsolatedStep(context);
        }
        returnedStderr = Optional.of(firstOrderStderr);
      } else {
        returnedStderr = Optional.empty();
      }

      if (declaredDepsBuildResult == StepExecutionResults.SUCCESS_EXIT_CODE && trackClassUsage) {
        AbsPath ruleCellRoot = context.getRuleCellRoot();
        RelPath outputJarDirPath = outputPaths.getOutputJarDirPath();
        ClassUsageFileWriterFactory.create(kotlincMode)
            .writeFile(
                KotlinClassUsageHelper.getClassUsageData(reportDirPath, ruleCellRoot),
                CompilerOutputPaths.getKotlinDepFilePath(outputJarDirPath),
                ruleCellRoot,
                configuredBuckOut);
      }

      return new StepExecutionResult(declaredDepsBuildResult, returnedStderr);
    } catch (IOException | InterruptedException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Compiles in [mode]. In an incremental round of an action that verifies Kosabi applicability,
   * [roundStateDir] is its incremental state, where {@link ApplicabilityRoundMarker} records the
   * compile; otherwise it is null.
   */
  private int compile(
      IsolatedExecutionContext context,
      IsolatedExecutionContext firstOrderContext,
      ImmutableList<String> compilerOptions,
      KotlincMode mode,
      KotlinCDLoggingContext loggingContext,
      @Nullable Path roundStateDir)
      throws InterruptedException, IOException {
    if (roundStateDir == null) {
      return compile(context, firstOrderContext, compilerOptions, mode, loggingContext);
    }
    ApplicabilityRoundMarker.enter(roundStateDir, ApplicabilityRoundMarker.Phase.COMPILING);
    int result = compile(context, firstOrderContext, compilerOptions, mode, loggingContext);
    ApplicabilityRoundMarker.leaveCompile(
        roundStateDir, result == StepExecutionResults.SUCCESS_EXIT_CODE);
    return result;
  }

  private @Nullable Path applicabilityRoundStateDir() {
    if (!verifySourceOnlyAbiConstraints
        || !invokingRule.isLibraryJar()
        || !(kotlincMode instanceof KotlincMode.Incremental)) {
      return null;
    }
    return ((KotlincMode.Incremental) kotlincMode).getKotlicWorkingDir().getPath().getParent();
  }

  private int compile(
      IsolatedExecutionContext context,
      IsolatedExecutionContext firstOrderContext,
      ImmutableList<String> compilerOptions,
      KotlincMode mode,
      KotlinCDLoggingContext loggingContext)
      throws InterruptedException {
    return kotlinc.buildWithClasspath(
        firstOrderContext,
        invokingRule,
        compilerOptions,
        kotlinHomeLibraries,
        sourceFilePaths,
        pathToSrcsList,
        Optional.of(outputPaths.getWorkingDirectory().getPath()),
        context.getRuleCellRoot(),
        mode,
        loggingContext,
        context.getRuleCellRoot().resolve(outputDirectory),
        emitsClassToSourceMap() && outputPaths.getCompilerClassToSourceMap() != null
            ? context
                .getRuleCellRoot()
                .resolve(outputPaths.getWorkingDirectory())
                .getPath()
                .resolve(CompilerClassToSourceMap.KOTLIN_FILE_NAME)
            : null);
  }

  /** A compile in a mode, returning its exit code. */
  @VisibleForTesting
  interface Compilation {
    int compile(KotlincMode mode) throws InterruptedException, IOException;
  }

  /** A copy of an incremental round's state, kept while a full compile replaces it. */
  @VisibleForTesting
  interface StateCopy {
    /** Puts the copied state back. */
    void restore() throws IOException;

    /** Drops the copy, keeping the current state. */
    void discard() throws IOException;
  }

  /** Copies an incremental round's state. */
  @VisibleForTesting
  interface StateCopier {
    StateCopy copy() throws IOException;
  }

  private static final StateCopy NO_COPY =
      new StateCopy() {
        @Override
        public void restore() {}

        @Override
        public void discard() {}
      };

  /**
   * Compiles in {@code mode}. When an incremental round of an action that verifies Kosabi
   * applicability compiled incrementally and failed on an applicability verdict alone, compiles
   * again in full and returns that compile's result: an incremental round analyses only the files
   * it recompiles, and checks that need facts only a source declaration carries treat unchanged
   * files conservatively, so a verdict that is not READY is final only after a full round.
   *
   * <p>The full compile replaces the incremental compiler's state. If it fails too, the state
   * copied before it is put back: the failed incremental compile left the last successful round's
   * state, so the next round can still be incremental.
   */
  @VisibleForTesting
  static int compileCheckingAgainInFull(
      boolean verifiesApplicability,
      KotlincMode mode,
      Compilation compilation,
      java.util.function.BooleanSupplier compiledIncrementally,
      Supplier<String> stderr,
      Runnable resetOutput,
      StateCopier stateCopier,
      String target)
      throws InterruptedException, IOException {
    boolean incrementalRound = isIncrementalApplicabilityRound(verifiesApplicability, mode);
    int result = compilation.compile(mode);
    if (!shouldCheckAgainInFull(
        incrementalRound && compiledIncrementally.getAsBoolean(), result, stderr)) {
      return result;
    }
    LOG.info(
        "Kosabi applicability did not pass an incremental round of %s; checking it in full",
        target);
    resetOutput.run();
    StateCopy copy;
    try {
      copy = stateCopier.copy();
    } catch (IOException e) {
      LOG.warn(e, "Could not copy the incremental state of %s", target);
      copy = NO_COPY;
    }
    int fullResult;
    try {
      fullResult =
          compilation.compile(
              ((KotlincMode.Incremental) mode)
                  .withRebuildReason(RebuildReason.APPLICABILITY_CHECKED_IN_FULL));
    } catch (InterruptedException | IOException | RuntimeException e) {
      try {
        copy.restore();
      } catch (IOException restoreFailure) {
        e.addSuppressed(restoreFailure);
      }
      throw e;
    }
    // The full compile's result stands either way: a copy left behind is replaced by the next one,
    // and state that cannot be put back is deleted, so the next round compiles in full.
    try {
      if (fullResult == StepExecutionResults.SUCCESS_EXIT_CODE) {
        copy.discard();
      } else {
        copy.restore();
      }
    } catch (IOException e) {
      LOG.warn(e, "Could not put back or drop the copy of the incremental state of %s", target);
    }
    return fullResult;
  }

  /** Copies the incremental compiler's working directory and the class outputs. */
  private StateCopy copyIncrementalState(AbsPath ruleCellRoot) throws IOException {
    Path workingDirectory = ((KotlincMode.Incremental) kotlincMode).getKotlicWorkingDir().getPath();
    Path stateDirectory = Objects.requireNonNull(workingDirectory.getParent());
    return IncrementalStateCopy.copy(
        stateDirectory.resolve(STATE_COPY_DIRECTORY),
        ImmutableList.of(workingDirectory, ruleCellRoot.resolve(outputDirectory).getPath()));
  }

  /**
   * Whether a round runs Kosabi applicability incrementally: it verifies source-only ABI
   * constraints, is not forced to be full, and has the shrunk classpath snapshot that only a
   * successful earlier round leaves, without which the incremental compiler compiles in full
   * anyway.
   */
  @VisibleForTesting
  static boolean isIncrementalApplicabilityRound(boolean verifiesApplicability, KotlincMode mode) {
    if (!verifiesApplicability || !(mode instanceof KotlincMode.Incremental)) {
      return false;
    }
    KotlincMode.Incremental incremental = (KotlincMode.Incremental) mode;
    return incremental.getRebuildReason() == null
        && Files.exists(
            incremental.getKotlicWorkingDir().resolve(SHRUNK_CLASSPATH_SNAPSHOT).getPath());
  }

  /**
   * Whether a round must be compiled again in full: an incremental applicability round that failed
   * on an applicability verdict alone. The plugin reports only when no other compiler error
   * occurred, and only for a verdict that is not READY.
   */
  @VisibleForTesting
  static boolean shouldCheckAgainInFull(
      boolean incrementalApplicabilityRound, int exitCode, Supplier<String> stderr) {
    return incrementalApplicabilityRound
        && exitCode != StepExecutionResults.SUCCESS_EXIT_CODE
        && stderr.get().contains(APPLICABILITY_REPORT_MARKER);
  }

  @VisibleForTesting
  Kotlinc getKotlinc() {
    return kotlinc;
  }

  @Override
  public String getIsolatedStepDescription(IsolatedExecutionContext context) {
    return getKotlinc()
        .getDescription(
            getOptions(context, getClasspathEntries()), sourceFilePaths, pathToSrcsList);
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
      IsolatedExecutionContext context, ImmutableList<AbsPath> buildClasspathEntries) {
    ImmutableList.Builder<String> builder = ImmutableList.builder();

    AbsPath ruleCellRoot = context.getRuleCellRoot();

    if (outputDirectory != null) {
      builder.add(DESTINATION_FLAG, ruleCellRoot.resolve(outputDirectory).toString());
    }

    if (invokingRule.isSourceOnlyAbi()) {
      configureSourceOnlyOptions(builder, languageVersion, ruleCellRoot);
    } else if (invokingRule.isSourceAbi()) {
      throw new Error("Source ABI flavor is not supported for Kotlin targets");
    } else if (!buildClasspathEntries.isEmpty()) {
      addClasspath(builder, buildClasspathEntries);
    }

    configureKosabiApplicability(builder, ruleCellRoot);

    if (trackClassUsage) {
      depTrackerPath.ifPresentOrElse(
          // New plugin, runtime dependency
          path -> {
            builder.add(X_PLUGIN_ARG + path);
          },
          // Fallback to a Buck internal plugin
          () -> builder.add(X_PLUGIN_ARG + PluginLoader.DEP_TRACKER_KOTLINC_PLUGIN_JAR_PATH));
      builder.add(PLUGIN);
      builder.add(
          "plugin:buck_deps_tracker:out="
              + ruleCellRoot.resolve(getKotlinTempDepFilePath(reportDirPath)));
    }

    if (!extraArguments.isEmpty()) {
      for (String extraArgument : extraArguments) {
        if (!extraArgument.isEmpty()) {
          builder.add(extraArgument);
        }
      }
    }

    if (languageVersion.getSupportsLanguageVersion()) {
      builder.add(languageVersion.getCompilerArgs());
    }

    if (context.getVerbosity().shouldUseVerbosityFlagIfAvailable()
        && !verboseModeOnlyExtraArguments.isEmpty()) {
      for (String extraArgument : verboseModeOnlyExtraArguments) {
        if (!extraArgument.isEmpty()) {
          builder.add(extraArgument);
        }
      }
    }

    return builder.build();
  }

  private void configureKosabiApplicability(
      ImmutableList.Builder<String> builder, AbsPath sourceRoot) {
    if (!verifySourceOnlyAbiConstraints || !invokingRule.isLibraryJar()) return;

    builder.add(
        X_PLUGIN_ARG + getRequiredKosabiApplicabilityPlugin(resolvedKosabiPluginOptionPath));
    builder.addAll(
        getKosabiApplicabilityPluginOptions(
            invokingRule.getFullyQualifiedName(),
            sourceRoot,
            applicabilityClasspath,
            Optional.ofNullable(
                resolvedKosabiPluginOptionPath.get(
                    KosabiConfig.PROPERTY_KOSABI_APPLICABILITY_CELL_ROOT))));
  }

  @VisibleForTesting
  static AbsPath getRequiredKosabiApplicabilityPlugin(
      ImmutableMap<String, AbsPath> resolvedPluginPaths) {
    AbsPath plugin = resolvedPluginPaths.get(KosabiConfig.PROPERTY_KOSABI_APPLICABILITY_PLUGIN);
    if (plugin == null) {
      throw new IllegalStateException(
          "Structured Kosabi applicability was enabled, but the plugin path is missing");
    }
    return plugin;
  }

  @VisibleForTesting
  static ImmutableList<String> getKosabiApplicabilityPluginOptions(
      String targetLabel,
      AbsPath sourceRoot,
      ImmutableList<AbsPath> classpath,
      Optional<AbsPath> cellRoot) {
    int cellSeparator = targetLabel.indexOf("//");
    if (cellSeparator <= 0) {
      throw new IllegalStateException(
          "Kosabi applicability requires a fully-qualified target label: " + targetLabel);
    }
    AbsPath resolvedCellRoot =
        cellRoot.orElseThrow(
            () -> new IllegalStateException("Kosabi applicability cell root path is missing"));
    if (!resolvedCellRoot.startsWith(sourceRoot)) {
      throw new IllegalStateException(
          "Kosabi applicability cell root path must be within the project source root");
    }
    String sourceRootPrefix =
        sourceRoot
            .relativize(resolvedCellRoot)
            .getPath()
            .toString()
            .replace(File.separatorChar, '/');
    if (sourceRootPrefix.isEmpty()) sourceRootPrefix = ".";

    ImmutableList.Builder<String> builder = ImmutableList.builder();
    addApplicabilityPluginOption(builder, "target-label", targetLabel);
    addApplicabilityPluginOption(builder, "source-root", sourceRoot.getPath().toString());
    addApplicabilityPluginOption(builder, "source-root-prefix", sourceRootPrefix);
    addApplicabilityPluginOption(
        builder,
        "source-only-abi-classpath",
        Joiner.on(File.pathSeparator)
            .join(transform(classpath, path -> path.getPath().toString())));
    return builder.build();
  }

  private static void addApplicabilityPluginOption(
      ImmutableList.Builder<String> builder, String name, String value) {
    builder.add(PLUGIN);
    builder.add("plugin:" + APPLICABILITY_PLUGIN_ID + ":" + name + "=" + value);
  }

  protected void configureSourceOnlyOptions(
      ImmutableList.Builder<String> builder,
      LanguageVersion languageVersion,
      AbsPath ruleCellRoot) {
    if (resolvedKosabiPluginOptionPath.containsKey(
        KosabiConfig.PROPERTY_KOSABI_STUBS_GEN_K2_PLUGIN)) {
      AbsPath stubPlugin =
          resolvedKosabiPluginOptionPath.get(KosabiConfig.PROPERTY_KOSABI_STUBS_GEN_K2_PLUGIN);
      builder.add(X_PLUGIN_ARG + stubPlugin);
    }

    if (shouldKosabiJvmAbiGenUseK2) {
      if (!resolvedKosabiPluginOptionPath.containsKey(
          KosabiConfig.PROPERTY_KOSABI_JVM_ABI_GEN_K2_PLUGIN)) {
        throw new RuntimeException("KosabiJvmAbiGenK2Plugin is not provided");
      }
      AbsPath jvmAbiPlugin =
          resolvedKosabiPluginOptionPath.get(KosabiConfig.PROPERTY_KOSABI_JVM_ABI_GEN_K2_PLUGIN);
      builder.add(X_PLUGIN_ARG + jvmAbiPlugin);
      builder.add(PLUGIN);
      builder.add(
          "plugin:com.facebook.k2.jvm.abi.gen:outputDir="
              + ruleCellRoot.resolve(outputDirectory).toString());
      // Enable Compose ABI emulation only when the Compose compiler plugin is active.
      // The emulation adds $stable fields and @StabilityInferred annotations to match
      // the real Compose compiler output. Only targets with the Compose plugin should
      // get these artifacts — otherwise @StabilityInferred leaks into ABI jars of
      // non-Compose targets and causes downstream javac failures.
      if (shouldEnableComposeAbiEmulation()) {
        builder.add(PLUGIN);
        builder.add("plugin:com.facebook.k2.jvm.abi.gen:enableComposeAbiEmulation=true");
      }
      if (shouldKosabiUseShallowClassUsage) {
        builder.add(PLUGIN);
        builder.add("plugin:com.facebook.k2.jvm.abi.gen:shallowClassUsage=true");
      }
      if (!kosabiAbiValidationMode.isEmpty() && !kosabiAbiValidationMode.equals("off")) {
        builder.add(PLUGIN);
        builder.add(
            "plugin:com.facebook.k2.jvm.abi.gen:abiValidationMode=" + kosabiAbiValidationMode);
      }
      for (String option : kosabiJvmAbiGenOptions) {
        builder.add(PLUGIN);
        builder.add("plugin:com.facebook.k2.jvm.abi.gen:" + option);
      }
      if (disableKosabiKeptBodyStubCheck) {
        builder.add(PLUGIN);
        builder.add("plugin:com.facebook.k2.jvm.abi.gen:keptBodyStubCheck=false");
      }
    }

    addClasspath(builder, this.compilationClasspath);
  }

  private boolean shouldEnableComposeAbiEmulation() {
    if (!shouldKosabiJvmAbiGenUseK2) return false;
    return extraArguments.stream()
        .filter(arg -> arg.startsWith("-Xplugin="))
        .anyMatch(arg -> arg.contains(COMPOSE_PLUGIN_PATH_FRAGMENT));
  }

  private void addClasspath(ImmutableList.Builder<String> builder, Iterable<AbsPath> pathElements) {
    builder.add(
        CLASSPATH_FLAG,
        Joiner.on(File.pathSeparator)
            .join(transform(pathElements, path -> path.getPath().toString())));
  }

  private boolean validateEarlyTerminationErrorMessage(
      IsolatedExecutionContext context, String fullErrorMessage) {
    if (fullErrorMessage.isEmpty()) return false;

    int earlyTerminationMessageIndex =
        fullErrorMessage.indexOf(Objects.requireNonNull(kosabiEarlyTerminationMessagePrefix));
    if (earlyTerminationMessageIndex >= 0) {
      return true;
    }
    return false;
  }

  /**
   * @return The classpath entries used to invoke javac.
   */
  @VisibleForTesting
  ImmutableList<AbsPath> getClasspathEntries() {
    return combinedClassPathEntries;
  }
}
