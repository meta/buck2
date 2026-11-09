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

import static com.facebook.buck.jvm.kotlin.ClasspathUtils.getClasspathSnapshots;
import static com.facebook.buck.jvm.kotlin.KosabiStubgenStepsBuilder.prepareKosabiStubgenIfNeeded;
import static com.facebook.buck.jvm.kotlin.KspStepsBuilder.prepareKspProcessorsIfNeeded;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.file.FileExtensionMatcher;
import com.facebook.buck.io.file.GlobPatternMatcher;
import com.facebook.buck.io.file.PathMatcher;
import com.facebook.buck.io.filesystem.CopySourceMode;
import com.facebook.buck.jvm.cd.command.kotlin.KotlinExtraParams;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.core.BuildTargetValueExtraParams;
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.buck.jvm.java.BaseCompileToJarStepFactory;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.CompilerOutputPathsValue;
import com.facebook.buck.jvm.java.CompilerParameters;
import com.facebook.buck.jvm.java.JarParameters;
import com.facebook.buck.jvm.java.JavacPluginParams;
import com.facebook.buck.jvm.java.ResolvedJavac;
import com.facebook.buck.jvm.java.ResolvedJavacOptions;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics;
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.CopyIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MakeCleanDirectoryIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MkdirIsolatedStep;
import com.facebook.buck.step.isolatedsteps.java.JarDirectoryStep;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableList.Builder;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSortedSet;
import java.nio.file.Path;
import java.util.Objects;
import java.util.stream.Collectors;
import javax.annotation.Nullable;

/**
 * Factory that creates Kotlin related compile build steps, but doesn't depend on any internal build
 * graph data structured. Intended to be used from the Daemon worker.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class DaemonKotlincToJarStepFactory extends BaseCompileToJarStepFactory<KotlinExtraParams> {
  static final PathMatcher KOTLIN_PATH_MATCHER = FileExtensionMatcher.of("kt");
  static final PathMatcher SRC_ZIP_MATCHER = GlobPatternMatcher.of("**.src.zip");

  private final KotlinCDAnalytics kotlinCDAnalytics;

  public DaemonKotlincToJarStepFactory(KotlinCDAnalytics kotlinCDAnalytics) {
    this.kotlinCDAnalytics = kotlinCDAnalytics;
  }

  @Override
  public KotlinExtraParams castExtraParams(ExtraParams extraParams) {
    return (KotlinExtraParams) extraParams;
  }

  @Override
  public void createCompileStep(
      RelPath buckOut,
      AbsPath buildCellRootPath,
      BuildTargetValue invokingRule,
      CompilerOutputPathsValue compilerOutputPathsValue,
      CompilerParameters parameters,
      Builder<IsolatedStep> steps,
      ResolvedJavac resolvedJavac,
      @Nullable ActionMetadata actionMetadata,
      KotlinExtraParams extraParams,
      @Nullable JarParameters abiJarParameters,
      boolean mixedCompilation) {

    Kotlinc kotlinc = KotlincFactory.create();

    CompilerOutputPaths compilerOutputPaths = parameters.getOutputPaths();
    BuildTargetValueExtraParams buildTargetValueExtraParams =
        BuildTargetValueExtraParams.of(invokingRule, compilerOutputPaths.getWorkingDirectory());

    ImmutableSortedSet<RelPath> sourceFilePaths = parameters.getSourceFilePaths();
    RelPath outputDirectory = compilerOutputPaths.getClassesDir();
    RelPath kotlinOutputDirectory = buildCellRootPath.relativize(extraParams.getKotlinClassesDir());
    if (!extraParams.getShouldActionRunIncrementally()) {
      steps.addAll(MakeCleanDirectoryIsolatedStep.of(kotlinOutputDirectory));
    } else {
      steps.add(new MkdirIsolatedStep(kotlinOutputDirectory));
    }
    RelPath annotationGenFolder = compilerOutputPaths.getAnnotationPath();
    Path pathToSrcsList = compilerOutputPaths.getPathToSourcesList().getPath();

    boolean hasKotlinSources =
        sourceFilePaths.stream().anyMatch(KOTLIN_PATH_MATCHER::matches)
            || sourceFilePaths.stream().anyMatch(SRC_ZIP_MATCHER::matches);

    ImmutableSortedSet.Builder<RelPath> sourceWithStubsAndKaptOutputBuilder =
        ImmutableSortedSet.orderedBy(RelPath.comparator()).addAll(sourceFilePaths);
    ImmutableSortedSet.Builder<RelPath> sourceWithStubsAndKaptAndKspOutputBuilder =
        ImmutableSortedSet.orderedBy(RelPath.comparator()).addAll(sourceFilePaths);
    ImmutableSortedSet.Builder<RelPath> javacSourceBuilder =
        ImmutableSortedSet.orderedBy(RelPath.comparator()).addAll(sourceFilePaths);

    steps.addAll(MakeCleanDirectoryIsolatedStep.of(annotationGenFolder));
    // Only invoke kotlinc if we have kotlin or src zip files.
    if (hasKotlinSources) {
      RelPath reportsOutput = buildTargetValueExtraParams.getAnnotationOutputPath("__%s_reports__");

      RelPath kotlincPluginGeneratedOutput =
          buildTargetValueExtraParams.getAnnotationOutputPath("__%s_kotlinc_plugin_generated__");

      // Javac requires that the root directory for generated sources already exist.
      steps.addAll(MakeCleanDirectoryIsolatedStep.of(kotlincPluginGeneratedOutput));
      steps.addAll(MakeCleanDirectoryIsolatedStep.of(reportsOutput));

      ClasspathUtils classpathUtils =
          new ClasspathUtils(
              buildCellRootPath,
              extraParams.getFriendPaths(),
              parameters.getClasspathEntries(),
              extraParams.getExtraClassPaths(),
              buildTargetValueExtraParams);

      String friendPathsArg = classpathUtils.getFriendPathArgs(steps);

      ImmutableList<AbsPath> allClasspaths = classpathUtils.getAllClasspaths(steps);

      ImmutableList<AbsPath> classpathSnapshots =
          extraParams.getShouldKotlincRunIncrementally()
              ? getClasspathSnapshots(
                  parameters,
                  steps,
                  buildCellRootPath,
                  allClasspaths,
                  extraParams.getExtraClassPathSnapshots())
              : ImmutableList.of();

      KosabiPluginOptions kosabiPluginOptions =
          new KosabiPluginOptions(extraParams.getKosabiPluginOptions());

      String moduleName = buildTargetValueExtraParams.getModuleName();
      String kotlinPluginGeneratedFullPath =
          buildCellRootPath.resolve(kotlincPluginGeneratedOutput).toString();

      Builder<IsolatedStep> postKotlinCompilationSteps = ImmutableList.builder();
      Builder<IsolatedStep> postKotlinCompilationFailureSteps = ImmutableList.builder();

      postKotlinCompilationSteps.add(
          CopyIsolatedStep.forDirectory(
              kotlincPluginGeneratedOutput,
              annotationGenFolder,
              CopySourceMode.DIRECTORY_CONTENTS_ONLY));

      JavacPluginParams annotationProcessorParams =
          extraParams.getResolvedJavacOptions().getJavaAnnotationProcessorParams();

      ImmutableList<AbsPath> kotlinHomeLibraries = extraParams.getKotlinHomeLibraries();

      KaptStepsBuilder.prepareKaptProcessorsIfNeeded(
          invokingRule,
          buildCellRootPath,
          steps,
          buildTargetValueExtraParams,
          extraParams.getJvmTarget(),
          extraParams.getStandardLibraryClassPath(),
          extraParams.getAnnotationProcessingClassPath(),
          outputDirectory,
          annotationGenFolder,
          javacSourceBuilder,
          reportsOutput,
          parameters.getShouldTrackClassUsage(),
          postKotlinCompilationSteps,
          annotationProcessorParams,
          extraParams.getExtraKotlincArguments(),
          sourceFilePaths,
          pathToSrcsList,
          allClasspaths,
          kotlinHomeLibraries,
          kotlinc,
          compilerOutputPaths,
          buckOut,
          extraParams.getKotlinCompilerPlugins(),
          kotlinPluginGeneratedFullPath,
          moduleName,
          kotlinCDAnalytics,
          sourceWithStubsAndKaptAndKspOutputBuilder,
          sourceWithStubsAndKaptOutputBuilder,
          extraParams.getLanguageVersion());
      ImmutableList.Builder<AbsPath> compilationClasspathBuilder =
          buildCompilationClasspath(parameters, extraParams);

      prepareKosabiStubgenIfNeeded(
          buckOut,
          buildCellRootPath,
          invokingRule,
          parameters,
          steps,
          extraParams,
          buildTargetValueExtraParams,
          sourceWithStubsAndKaptOutputBuilder,
          sourceWithStubsAndKaptAndKspOutputBuilder,
          outputDirectory,
          sourceFilePaths,
          pathToSrcsList,
          allClasspaths,
          reportsOutput,
          kotlinc,
          kosabiPluginOptions.getAllKosabiPlugins(),
          compilationClasspathBuilder,
          postKotlinCompilationFailureSteps,
          kotlinCDAnalytics,
          extraParams.getLanguageVersion());

      prepareKspProcessorsIfNeeded(
          extraParams,
          invokingRule,
          buildCellRootPath,
          steps,
          postKotlinCompilationSteps,
          buildTargetValueExtraParams,
          outputDirectory,
          annotationGenFolder,
          javacSourceBuilder,
          reportsOutput,
          parameters.getShouldTrackClassUsage(),
          allClasspaths,
          kotlinPluginGeneratedFullPath,
          annotationProcessorParams,
          sourceWithStubsAndKaptOutputBuilder.build(),
          compilerOutputPaths,
          sourceWithStubsAndKaptAndKspOutputBuilder,
          compilationClasspathBuilder.build(),
          moduleName,
          kotlinCDAnalytics);

      // Reduced SO-ABI classpath for the applicability plugin (rfsoa +
      // source_only_abi_deps only). Distinct from compilationClasspath which
      // contains the full dep set during library builds.
      ImmutableList<AbsPath> applicabilityClasspath = extraParams.getApplicabilityClasspath();

      KotlinCStepsBuilder.prepareKotlinCompilation(
          buckOut,
          buildCellRootPath,
          invokingRule,
          parameters,
          steps,
          actionMetadata,
          extraParams,
          friendPathsArg,
          kotlinPluginGeneratedFullPath,
          moduleName,
          kotlinOutputDirectory,
          sourceWithStubsAndKaptAndKspOutputBuilder,
          pathToSrcsList,
          allClasspaths,
          reportsOutput,
          kotlinc,
          kosabiPluginOptions,
          compilationClasspathBuilder.build(),
          applicabilityClasspath,
          postKotlinCompilationFailureSteps,
          classpathSnapshots,
          kotlinCDAnalytics);
      steps.addAll(postKotlinCompilationSteps.build());
    }

    ResolvedJavacOptions resolvedJavacOptions = extraParams.getResolvedJavacOptions();
    if (hasKotlinSources) {
      // Most of the time, KotlinC have ran annotation processing,
      // so only run "java on mix" processors (very uncommon) on Javac
      resolvedJavacOptions =
          resolvedJavacOptions.withJavaAnnotationProcessorParams(
              getRunsOnJavaOnlyProcessors(resolvedJavacOptions));
    }

    JavacStepsBuilder.prepareJavaCompilationIfNeeded(
        invokingRule,
        buildCellRootPath,
        steps,
        buckOut,
        compilerOutputPathsValue,
        parameters,
        resolvedJavac,
        resolvedJavacOptions,
        parameters.getClasspathEntries(),
        extraParams.getExtraClassPaths(),
        extraParams.getApplicabilityClasspath(),
        hasKotlinSources
            ? ImmutableList.of(kotlinOutputDirectory, outputDirectory)
            : ImmutableList.of(outputDirectory),
        javacSourceBuilder,
        abiJarParameters);
  }

  @Override
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
      KotlinExtraParams extraParams) {

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
        abiJarParameters,
        true);
    steps.add(
        new JarDirectoryStep(
            Objects.requireNonNull(
                abiJarParameters == null ? libraryJarParameters : abiJarParameters),
            ImmutableSet.of(extraParams.getKotlinClassesDir())));
  }

  /**
   * Retrieve Java only processors. They should run on javac even in kotlin modules. This means they
   * would only take effect on java files in mix modules
   */
  static JavacPluginParams getRunsOnJavaOnlyProcessors(ResolvedJavacOptions resolvedJavacOptions) {
    JavacPluginParams javaAnnotationProcessorParams =
        resolvedJavacOptions.getJavaAnnotationProcessorParams();
    ImmutableList<ResolvedJavacPluginProperties> filteredPluginProperties =
        javaAnnotationProcessorParams.getPluginProperties().stream()
            .filter(AnnotationProcessorUtils::isRunsOnJavaOnlyProcessor)
            .collect(ImmutableList.toImmutableList());
    // See https://fburl.com/diff/d1msdqm8
    // If pluginProperties is empty, make sure parameters is empty too, or javac will complain
    if (filteredPluginProperties.isEmpty()) {
      return JavacPluginParams.EMPTY;
    }
    return new JavacPluginParams(
        filteredPluginProperties, javaAnnotationProcessorParams.getParameters());
  }

  /**
   * Builds the compilation classpath by combining regular classpath entries with the bootclasspath
   * (which includes android.jar for Android targets). For library builds this is the full dep set;
   * for SO-ABI builds it is the reduced set (rfsoa deps only).
   *
   * @param parameters Compiler parameters containing classpath entries
   * @param extraParams Kotlin-specific parameters containing resolved javac options with
   *     bootclasspath
   * @return A builder containing all classpath entries (regular + bootclasspath) as absolute paths
   */
  static ImmutableList.Builder<AbsPath> buildCompilationClasspath(
      CompilerParameters parameters, KotlinExtraParams extraParams) {
    ImmutableList.Builder<AbsPath> compilationClasspathBuilder =
        ImmutableList.<AbsPath>builder()
            .addAll(
                parameters.getClasspathEntries().stream()
                    .map(RelPath::toAbsolutePath)
                    .filter(ClasspathUtils::assertValidClasspathsPattern)
                    .collect(Collectors.toList()));

    compilationClasspathBuilder.addAll(
        extraParams.getResolvedJavacOptions().getBootclasspathList().stream()
            .map(RelPath::toAbsolutePath)
            .filter(ClasspathUtils::assertValidClasspathsPattern)
            .collect(Collectors.toList()));

    return compilationClasspathBuilder;
  }
}
