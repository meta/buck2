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

import static com.facebook.buck.jvm.java.JavaPaths.SRC_ZIP;
import static com.facebook.buck.jvm.kotlin.AnnotationProcessorUtils.getAnnotationProcessors;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.io.filesystem.CopySourceMode;
import com.facebook.buck.jvm.cd.command.kotlin.KotlinExtraParams;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.core.BuildTargetValueExtraParams;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.java.JavacPluginParams;
import com.facebook.buck.jvm.java.ResolvedJavacPluginProperties;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics;
import com.facebook.buck.jvm.kotlin.ksp.Ksp2Step;
import com.facebook.buck.step.isolatedsteps.IsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.CopyIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MakeCleanDirectoryIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.MkdirIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.RmIsolatedStep;
import com.facebook.buck.step.isolatedsteps.common.ZipIsolatedStep;
import com.facebook.buck.util.zip.ZipCompressionLevel;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSet;
import com.google.common.collect.ImmutableSortedSet;
import java.util.List;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KspStepsBuilder {
  /**
   * Providers of the processors that may write the final-round placeholder: the triggers of the
   * shared KSP-run protocol (`//fbandroid/java/com/facebook/kotlin/ksp/runprotocol`). Keyed by
   * provider name, so a renamed or new trigger provider must be added here.
   */
  private static final ImmutableSet<String> PROCESSORS_USING_FINAL_ROUND_PLACEHOLDER =
      ImmutableSet.of(
          "KSP:com.facebook.annotationprocessors.inject.ksp.InjectorKspProcessorProvider",
          "KSP:com.facebook.metagen.processor.kspmetagen.MetagenKspProcessorProvider");

  /** Initialize all the folders, steps and parameters needed to run KSP plugins for this rule. */
  public static void prepareKspProcessorsIfNeeded(
      KotlinExtraParams extraParams,
      BuildTargetValue invokingRule,
      AbsPath rootPath,
      ImmutableList.Builder<IsolatedStep> steps,
      ImmutableList.Builder<IsolatedStep> postKotlinCompilationSteps,
      BuildTargetValueExtraParams buildTargetValueExtraParams,
      RelPath outputDirectory,
      RelPath annotationGenFolder,
      ImmutableSortedSet.Builder<RelPath> javacSourceBuilder,
      RelPath reportsOutput,
      boolean shouldTrackClassUsage,
      ImmutableList<AbsPath> allClasspaths,
      String kotlinPluginGeneratedOutFullPath,
      JavacPluginParams annotationProcessorParams,
      ImmutableSortedSet<RelPath> sourceFilePaths,
      CompilerOutputPaths compilerOutputPaths,
      ImmutableSortedSet.Builder<RelPath> sourceBuilderWithKspOutputs,
      ImmutableList<AbsPath> compilationClasspath,
      String moduleName,
      KotlinCDAnalytics kotlinCDAnalytics) {

    ImmutableList<ResolvedJavacPluginProperties> kspAnnotationProcessors =
        getKspAnnotationProcessors(getAnnotationProcessors(annotationProcessorParams));

    // We need to generate the KSP generation folder anyway, to help IntelliJ with red
    // symbols.
    RelPath kspAnnotationGenFolder = buildTargetValueExtraParams.getKspAnnotationGenPath();
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kspAnnotationGenFolder));

    if (kspAnnotationProcessors.isEmpty()) {
      return;
    }

    steps.add(new MkdirIsolatedStep(outputDirectory));
    // KSP folders
    RelPath kspOutputBaseDir = buildTargetValueExtraParams.getAnnotationOutputBasePath();
    RelPath kspClassesOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_ksp_classes__");
    RelPath kspKotlinOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_ksp_generated_kotlin__");
    RelPath kspJavaOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_ksp_generated_java__");
    RelPath kspCachesOutput =
        buildTargetValueExtraParams.getAnnotationOutputPath("__%s_ksp_cache_output__");

    // More KSP folders
    RelPath kspGenOutputFolder = buildTargetValueExtraParams.getGenPath("__%s_ksp_gen_sources__");
    RelPath kspGenOutput =
        buildTargetValueExtraParams.getGenPath("__%s_ksp_gen_sources__/generated" + SRC_ZIP);

    // Creating KSP dirs
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kspClassesOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kspKotlinOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kspJavaOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kspCachesOutput));
    steps.addAll(MakeCleanDirectoryIsolatedStep.of(kspGenOutputFolder));

    ImmutableList<String> kspProcessorsClasspathList =
        kspAnnotationProcessors.stream()
            .map(p -> p.toUrlClasspath(rootPath))
            .flatMap(List::stream)
            .map(AnnotationProcessorUtils::urlToFile)
            .collect(ImmutableList.toImmutableList());

    ImmutableList.Builder<AbsPath> allClassPathsBuilder = ImmutableList.builder();

    allClassPathsBuilder.addAll(allClasspaths);

    if (invokingRule.isSourceOnlyAbi()) {
      allClassPathsBuilder.addAll(
          compilationClasspath.stream().filter(p -> !allClasspaths.contains(p)).toList());
    }

    Ksp2Step ksp2Step =
        new Ksp2Step(
            invokingRule,
            compilerOutputPaths,
            rootPath,
            shouldTrackClassUsage,
            allClassPathsBuilder.build(),
            kotlinPluginGeneratedOutFullPath,
            annotationProcessorParams.getParameters(),
            sourceFilePaths,
            CompilerOutputPaths.getKspDepFilePath(reportsOutput),
            moduleName,
            kspProcessorsClasspathList,
            kspClassesOutput,
            kspKotlinOutput,
            kspJavaOutput,
            kspOutputBaseDir,
            extraParams.getJvmTarget(),
            extraParams.getLanguageVersion(),
            getJvmDefaultMode(extraParams.getExtraKotlincArguments()),
            extraParams.getJavaBinary(),
            kotlinCDAnalytics,
            kspCachesOutput,
            extraParams.getFriendPaths());
    steps.add(ksp2Step);
    steps.addAll(
        createKspOutputStagingSteps(
            rootPath,
            kspKotlinOutput,
            kspJavaOutput,
            kspClassesOutput,
            kspAnnotationGenFolder,
            kspGenOutput,
            annotationGenFolder,
            kspAnnotationProcessors));

    // Generated classes should be part of the output. This way generated files such as
    // META-INF dirs will also be added to the final jar.
    postKotlinCompilationSteps.add(
        CopyIsolatedStep.forDirectory(
            kspClassesOutput.getPath(),
            outputDirectory.getPath(),
            CopySourceMode.DIRECTORY_CONTENTS_ONLY));

    sourceBuilderWithKspOutputs.add(kspGenOutput);

    javacSourceBuilder.add(kspGenOutput);
  }

  static ImmutableList<IsolatedStep> createKspOutputStagingSteps(
      AbsPath rootPath,
      RelPath kspKotlinOutput,
      RelPath kspJavaOutput,
      RelPath kspClassesOutput,
      RelPath kspAnnotationGenFolder,
      RelPath kspGenOutput,
      RelPath annotationGenFolder,
      ImmutableList<ResolvedJavacPluginProperties> kspAnnotationProcessors) {
    ImmutableList.Builder<IsolatedStep> stagingSteps = ImmutableList.builder();
    stagingSteps.add(
        CopyIsolatedStep.forDirectory(
            kspKotlinOutput, kspAnnotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));
    stagingSteps.add(
        CopyIsolatedStep.forDirectory(
            kspJavaOutput, kspAnnotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));
    stagingSteps.add(
        CopyIsolatedStep.forDirectory(
            kspClassesOutput, kspAnnotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));

    if (usesFinalRoundPlaceholder(kspAnnotationProcessors)) {
      // A KSP-run protocol trigger writes this empty source only to force KSP's final round.
      // Keep it in KSP's own output for round and incremental bookkeeping, but exclude it from the
      // staged generated sources consumed by Kotlin and javac.
      stagingSteps.add(
          new RmIsolatedStep(
              kspAnnotationGenFolder.resolveRel("com/facebook/Dummy.java"),
              false,
              ImmutableSet.of()));
    }

    stagingSteps.add(
        new ZipIsolatedStep(
            rootPath,
            kspGenOutput.getPath(),
            ImmutableSet.of(),
            ImmutableSet.of(),
            false,
            ZipCompressionLevel.DEFAULT,
            kspAnnotationGenFolder.getPath()));
    stagingSteps.add(
        CopyIsolatedStep.forDirectory(
            kspAnnotationGenFolder, annotationGenFolder, CopySourceMode.DIRECTORY_CONTENTS_ONLY));
    return stagingSteps.build();
  }

  static boolean usesFinalRoundPlaceholder(
      ImmutableList<ResolvedJavacPluginProperties> kspAnnotationProcessors) {
    return kspAnnotationProcessors.stream()
        .flatMap(processor -> processor.getProcessorNames().stream())
        .anyMatch(PROCESSORS_USING_FINAL_ROUND_PLACEHOLDER::contains);
  }

  private static String getJvmDefaultMode(ImmutableList<String> args) {
    for (String arg : args) {
      String[] splitArg = arg.split("=");
      if (splitArg.length == 2) {
        if (splitArg[0].equals("-Xjvm-default")) {
          return splitArg[1];
        }
        // Kotlin 2.3 renamed -Xjvm-default to -jvm-default with new mode names. Map them back to
        // the legacy mode names KSP expects to preserve behavior.
        if (splitArg[0].equals("-jvm-default")) {
          switch (splitArg[1]) {
            case "no-compatibility":
              return "all";
            case "enable":
              return "all-compatibility";
            case "disable":
              return "disable";
            default:
              return splitArg[1];
          }
        }
      }
    }
    return "disabled";
  }

  private static boolean isKspPlugin(AbsPath sourcePath) {
    return sourcePath.toString().contains("symbol-processing");
  }

  public static boolean isNotKspPlugin(AbsPath sourcePath, ImmutableMap<String, String> options) {
    return !isKspPlugin(sourcePath);
  }

  static ImmutableList<ResolvedJavacPluginProperties> getKspAnnotationProcessors(
      ImmutableList<ResolvedJavacPluginProperties> annotationProcessors) {
    return annotationProcessors.stream()
        .filter(AnnotationProcessorUtils::isKSPProcessor)
        .collect(ImmutableList.toImmutableList());
  }
}
