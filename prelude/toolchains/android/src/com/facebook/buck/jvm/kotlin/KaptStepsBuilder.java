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

import static com.facebook.buck.jvm.java.CompilerOutputPaths.getKAPTDepFilePath;
import static com.facebook.buck.jvm.java.JavaPaths.SRC_ZIP;
import static com.facebook.buck.jvm.kotlin.AnnotationProcessorUtils.encodeOptions;
import static com.facebook.buck.jvm.kotlin.AnnotationProcessorUtils.isKSPProcessor;
import static com.facebook.buck.jvm.kotlin.AnnotationProcessorUtils.isRunsOnJavaOnlyProcessor;
import static com.facebook.buck.jvm.kotlin.AnnotationProcessorUtils.urlToFile;
import static com.facebook.buck.jvm.kotlin.CompilerPluginUtils.MODULE_NAME;
import static com.facebook.buck.jvm.kotlin.CompilerPluginUtils.getKotlinCompilerPluginsArgs;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.CopySourceMode;
import com.facebook.buck.jvm.cd.command.kotlin.KotlinSupportedLanguageVersion;
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.core.BuildTargetValueExtraParams;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.JavacPluginParams;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics;
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.CopyIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MakeCleanDirectoryIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.ZipIsolatedStep;
import com.facebook.buck.util.zip.ZipCompressionLevel;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.base.Joiner;
import com.google.common.base.Preconditions;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSortedSet;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.stream.Collectors;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KaptStepsBuilder {

  private static final String KAPT3_PLUGIN = "plugin:org.jetbrains.kotlin.kapt3:";
  private static final String AP_CLASSPATH_ARG = KAPT3_PLUGIN + "apclasspath=";
  private static final String AP_PROCESSORS_ARG = KAPT3_PLUGIN + "processors=";
  // output path for generated sources;
  private static final String SOURCES_ARG = KAPT3_PLUGIN + "sources=";
  private static final String CLASSES_ARG = KAPT3_PLUGIN + "classes=";
  // output path for java stubs;
  private static final String STUBS_ARG = KAPT3_PLUGIN + "stubs=";
  private static final String LIGHT_ANALYSIS = KAPT3_PLUGIN + "useLightAnalysis=";
  private static final String CORRECT_ERROR_TYPES = KAPT3_PLUGIN + "correctErrorTypes=";
  private static final String JAVAC_ARG = KAPT3_PLUGIN + "javacArguments=";
  private static final String AP_OPTIONS = KAPT3_PLUGIN + "apoptions=";
  private static final String AP_FILE_ACCESS_HIST_REPORT_ARG =
      KAPT3_PLUGIN + "dumpFileReadHistory=";
  private static final String KAPT_USE_K2 = KAPT3_PLUGIN + "useK2=";
  private static final String KAPT_GENERATED = "kapt.kotlin.generated";
  public static final String PLUGIN = "-P";
  public static final String AP_STATS_REPORT_FILE = "ap_stats.report";
  public static final String AP_ERROR_REPORT_PATH = "ap_error.report";
  public static final String AP_ERROR_REPORT_OPTION_NAME = "error_report";
  public static final String APT_MODE = "aptMode=";
  public static final String X_PLUGIN_ARG = "-Xplugin=";
  private static final String KOTLINC_KAPT_USE_USE_KAPT4_OLD = "-Xuse-kapt4";
  private static final String KOTLINC_KAPT_USE_K2 = "-Xuse-k2-kapt";

  /**
   * Creates the necessary steps, folders and parameters needed to run annotation processors using
   * KAPT.
   *
   * <p>This method will do nothing if there are no relevant annotation processors to run.
   */
  public static void prepareKaptProcessorsIfNeeded(
      BuildTargetValue invokingRule,
      AbsPath rootCellPath,
      ImmutableList.Builder<IsolatedStep> steps,
      BuildTargetValueExtraParams buildTargetValueExtraParams,
      Optional<String> jvmTarget,
      AbsPath resolvedKotlinStandardLibraryClassPath,
      AbsPath resolvedAnnotationProcessingClassPath,
      RelPath outputDirectory,
      RelPath annotationGenFolder,
      ImmutableSortedSet.Builder<RelPath> javacSourceBuilder,
      RelPath reportsOutput,
      boolean shouldTrackClassUsage,
      ImmutableList.Builder<IsolatedStep> postKotlinCompilationSteps,
      JavacPluginParams javaAnnotationProcessorParams,
      ImmutableList<String> extraKotlincArguments,
      ImmutableSortedSet<RelPath> sourceFilePaths,
      Path pathToSrcsList,
      ImmutableList<AbsPath> allClasspaths,
      ImmutableList<AbsPath> kotlinHomeLibraries,
      Kotlinc kotlinc,
      CompilerOutputPaths compilerOutputPaths,
      RelPath configuredBuckOut,
      ImmutableMap<AbsPath, ImmutableMap<String, String>> resolvedKotlinCompilerPlugins,
      String kotlinPluginGeneratedOutFullPath,
      String moduleName,
      KotlinCDAnalytics kotlinCDAnalytics,
      ImmutableSortedSet.Builder<RelPath> sourceWithStubsAndKaptAndKspOutputBuilder,
      ImmutableSortedSet.Builder<RelPath> sourceWithStubsAndKaptOutputBuilder,
      LanguageVersion kotlinLanguageVersion) {
    // We don't need the Kapt processor to run for source-only-abi
    if (invokingRule.isSourceOnlyAbi()) {
      return;
    }

    // We need to generate the KAPT generation folder anyway, to help IntelliJ with red symbols.
    RelPath kaptAnnotationGenFolder = buildTargetValueExtraParams.getKaptAnnotationGenPath();
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kaptAnnotationGenFolder));

    ImmutableList<ResolvedJavacPluginProperties> kaptAnnotationProcessors =
        getKaptAnnotationProcessors(
            AnnotationProcessorUtils.getAnnotationProcessors(javaAnnotationProcessorParams));

    // No annotation processors for KAPT
    if (kaptAnnotationProcessors.isEmpty()) {
      return;
    }

    // KAPT folders
    RelPath kaptSourcesOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_kapt_sources__");
    RelPath kaptClassesOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_kapt_classes__");
    RelPath stubsOutput = buildTargetValueExtraParams.getAnnotationOutputPath("__%s_kapt_stubs__");
    RelPath kaptGeneratedOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_kapt_generated__");
    RelPath kaptGenOutputFolder = buildTargetValueExtraParams.getGenPath("__%s_kapt_gen_sources__");
    RelPath kaptGenOutput =
        buildTargetValueExtraParams.getGenPath("__%s_kapt_gen_sources__/generated" + SRC_ZIP);

    // Creating KAPT dirs
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kaptClassesOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kaptSourcesOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(stubsOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kaptGeneratedOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kaptGenOutputFolder));

    ImmutableList<String> kaptProcessorsArg =
        ImmutableList.copyOf(
            kaptAnnotationProcessors.stream()
                .map(ResolvedJavacPluginProperties::getProcessorNames)
                .flatMap(Set::stream)
                .map(name -> AP_PROCESSORS_ARG + name)
                .collect(Collectors.toList()));

    ImmutableList<String> kaptPluginsClasspath =
        ImmutableList.copyOf(
            kaptAnnotationProcessors.stream()
                .map(p -> p.toUrlClasspath(rootCellPath))
                .flatMap(List::stream)
                .map(url -> AP_CLASSPATH_ARG + rootCellPath.resolve(urlToFile(url)))
                .collect(Collectors.toList()));

    ImmutableMap.Builder<String, String> apOptions = new ImmutableMap.Builder<>();

    ImmutableSortedSet<String> javacAnnotationProcessorParams =
        javaAnnotationProcessorParams.getParameters();
    for (String param : javacAnnotationProcessorParams) {
      String[] splitParam = param.split("=");
      Preconditions.checkState(splitParam.length == 2);
      apOptions.put(splitParam[0], splitParam[1]);
    }
    Path annotationProcessorsStatsFilePath =
        rootCellPath.resolve(reportsOutput).resolve(AP_STATS_REPORT_FILE).getPath();

    // write annotation processor error reports to this path
    Path annotationProcessorsErrorReportPath =
        rootCellPath.resolve(reportsOutput).resolve(AP_ERROR_REPORT_PATH).getPath();
    apOptions.put(AP_ERROR_REPORT_OPTION_NAME, annotationProcessorsErrorReportPath.toString());

    ImmutableList.Builder<String> kaptPluginOptionsBuilder =
        ImmutableList.<String>builder()
            .add(
                AP_CLASSPATH_ARG
                    + rootCellPath.resolve(resolvedAnnotationProcessingClassPath.getPath()))
            .add(
                AP_CLASSPATH_ARG
                    + rootCellPath.resolve(resolvedKotlinStandardLibraryClassPath.getPath()))
            .addAll(kaptPluginsClasspath)
            .addAll(kaptProcessorsArg)
            .add(SOURCES_ARG + rootCellPath.resolve(kaptSourcesOutput))
            .add(CLASSES_ARG + rootCellPath.resolve(kaptClassesOutput))
            .add(STUBS_ARG + rootCellPath.resolve(stubsOutput))
            .add(
                AP_OPTIONS
                    + encodeKaptApOptions(
                        apOptions.build(), rootCellPath.resolve(kaptGeneratedOutput).toString()))
            .add(JAVAC_ARG + encodeOptions(getJavacArguments(jvmTarget)))
            .add(LIGHT_ANALYSIS + "true") // TODO: Provide value as argument
            .add(CORRECT_ERROR_TYPES + "true");

    if (isKapt4SupportedForCurrentKotlinLanguageVersion(kotlinLanguageVersion)) {
      kaptPluginOptionsBuilder.add(KAPT_USE_K2 + "true");
    }

    if (shouldTrackClassUsage) {
      kaptPluginOptionsBuilder.add(
          AP_FILE_ACCESS_HIST_REPORT_ARG + rootCellPath.resolve(getKAPTDepFilePath(reportsOutput)));
    }
    ImmutableList.Builder<String> annotationProcessingOptionsBuilder =
        ImmutableList.<String>builder()
            .add(X_PLUGIN_ARG + resolvedAnnotationProcessingClassPath)
            .add(PLUGIN)
            .add(
                KAPT3_PLUGIN
                    + APT_MODE
                    + "stubsAndApt,"
                    + Joiner.on(",").join(kaptPluginOptionsBuilder.build()))
            .add(MODULE_NAME)
            .add(moduleName);

    // Use kapt4 or kapt3 base on language version is 1.9 or 2.0+
    if (isKapt4SupportedForCurrentKotlinLanguageVersion(kotlinLanguageVersion)) {
      // Notice K2 and KAPT4 can no longer use K1 DI plugin, so we no longer pass it
      // What it miss is the DI plugin's analysis part, which is needed BEFORE annotation
      // processing, the main KotlincStep still get k2 DI transformation plugin
      // Because of that, KAPT+DI should be avoided and migrate to KSP+DI ASAP,
      // and modules who still use KAPT+DI must turn off k2 and fallback to k1 and kapt3.
      annotationProcessingOptionsBuilder.add(getKapt4Flag(kotlinLanguageVersion));
      annotationProcessingOptionsBuilder.addAll(
          getOtherPluginsRequiredForKapt4(
              resolvedKotlinCompilerPlugins, kotlinPluginGeneratedOutFullPath));
    } else {
      annotationProcessingOptionsBuilder.addAll(
          getOtherPluginsRequiredForKapt3(
              resolvedKotlinCompilerPlugins, kotlinPluginGeneratedOutFullPath));
    }

    steps.add(
        new KaptStep(
            invokingRule,
            outputDirectory.getPath(),
            sourceFilePaths,
            pathToSrcsList,
            allClasspaths,
            kotlinHomeLibraries,
            reportsOutput,
            kotlinc,
            annotationProcessingOptionsBuilder.build(),
            compilerOutputPaths,
            configuredBuckOut,
            kotlinCDAnalytics,
            kotlinLanguageVersion));

    steps.add(
        CopyIsolatedStep.forDirectory(
            kaptSourcesOutput, kaptAnnotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));
    steps.add(
        CopyIsolatedStep.forDirectory(
            kaptClassesOutput, kaptAnnotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));
    steps.add(
        CopyIsolatedStep.forDirectory(
            kaptGeneratedOutput, kaptAnnotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));

    steps.add(
        new ZipIsolatedStep(
            rootCellPath,
            kaptGenOutput.getPath(),
            ImmutableSet.of(),
            ImmutableSet.of(),
            false,
            ZipCompressionLevel.DEFAULT,
            kaptAnnotationGenFolder.getPath()));

    steps.add(
        CopyIsolatedStep.forDirectory(
            kaptAnnotationGenFolder, annotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));

    // Generated classes should be part of the output. This way generated files
    // such as META-INF dirs will also be added to the final jar.
    postKotlinCompilationSteps.add(
        CopyIsolatedStep.forDirectory(
            kaptClassesOutput.getPath(),
            outputDirectory.getPath(),
            CopySourceMode.DIRECTORY_CONTENTS_ONLY));

    sourceWithStubsAndKaptAndKspOutputBuilder.add(kaptGenOutput);
    sourceWithStubsAndKaptOutputBuilder.add(kaptGenOutput);
    javacSourceBuilder.add(kaptGenOutput);
  }

  private static String getKapt4Flag(LanguageVersion kotlinLanguageVersion) {
    if (kotlinLanguageVersion.isGreaterOrEqual(KotlinSupportedLanguageVersion.V2_1)) {
      return KOTLINC_KAPT_USE_K2;
    }

    return KOTLINC_KAPT_USE_USE_KAPT4_OLD;
  }

  /**
   * KAPT4 should only be turned on for k2 targets. Targets that can't use k2 or can't use kapt4
   * shall use [k2=False] which ensure they get [-language-version=1.9] and fail the condition here.
   */
  public static boolean isKapt4SupportedForCurrentKotlinLanguageVersion(
      LanguageVersion kotlinLanguageVersion) {
    return kotlinLanguageVersion.getSupportsK2();
  }

  static ImmutableList<ResolvedJavacPluginProperties> getKaptAnnotationProcessors(
      ImmutableList<ResolvedJavacPluginProperties> annotationProcessors) {
    return annotationProcessors.stream()
        .filter(prop -> !isKSPProcessor(prop) && !isRunsOnJavaOnlyProcessor(prop))
        .collect(ImmutableList.toImmutableList());
  }

  private static String encodeKaptApOptions(
      Map<String, String> kaptApOptions, String kaptGeneratedPath) {
    Map<String, String> kaptApOptionsToEncode = new HashMap<>();
    kaptApOptionsToEncode.put(KAPT_GENERATED, kaptGeneratedPath);
    kaptApOptionsToEncode.putAll(kaptApOptions);

    return encodeOptions(kaptApOptionsToEncode);
  }

  private static Map<String, String> getJavacArguments(Optional<String> jvmTarget) {
    Map<String, String> arguments = new HashMap<>();
    jvmTarget.ifPresent(
        target -> {
          arguments.put("-source", target);
          arguments.put("-target", target);
        });
    return arguments;
  }

  /**
   * Plugins required to run on the KotlincStep for KAPT3, excluding kapt3 itself. So far it's DI
   * and Kotlin all-open, because (1) KAPT+DI need DI K1 plugin to do an analysis and codegen before
   * it, (2) kotlin all-open also need to run before KAPT to avoid crashing during KAPT
   */
  private static ImmutableList<String> getOtherPluginsRequiredForKapt3(
      ImmutableMap<AbsPath, ImmutableMap<String, String>> resolvedKotlinCompilerPlugins,
      String outputDir) {
    return getKotlinCompilerPluginsArgs(
        resolvedKotlinCompilerPlugins,
        outputDir,
        (sourcePath, pluginOptions) ->
            CompilerPluginUtils.isDiK1PluginForKapt(sourcePath, pluginOptions)
                || CompilerPluginUtils.isKotlinAllOpenPlugin(sourcePath, pluginOptions));
  }

  /**
   * Plugins required to run on the KotlincStep for KAPT4, excluding kapt4 itself. So far just
   * Kotlin all-open. Notice DI K2 plugin only kept the transform functionality, which is only
   * useful in main KotlincStep, no need to run on KAPT KotlincStep.
   */
  private static ImmutableList<String> getOtherPluginsRequiredForKapt4(
      ImmutableMap<AbsPath, ImmutableMap<String, String>> resolvedKotlinCompilerPlugins,
      String outputDir) {
    return getKotlinCompilerPluginsArgs(
        resolvedKotlinCompilerPlugins, outputDir, CompilerPluginUtils::isKotlinAllOpenPlugin);
  }
}
