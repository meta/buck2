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

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion;
import com.facebook.buck.jvm.core.BuildTargetValue;
import com.facebook.buck.jvm.java.CompilerOutputPaths;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics;
import com.facebook.buck.jvm.kotlin.kotlinc.Kotlinc;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import com.google.common.collect.ImmutableSortedSet;
import java.nio.file.Path;
import java.util.Optional;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KaptStep extends KotlincStep {

  private static final String VERBOSE = "-verbose";

  KaptStep(
      BuildTargetValue invokingRule,
      Path outputDirectory,
      ImmutableSortedSet<RelPath> sourceFilePaths,
      Path pathToSrcsList,
      ImmutableList<AbsPath> combinedClassPathEntries,
      ImmutableList<AbsPath> kotlinHomeLibraries,
      RelPath reportDirPath,
      Kotlinc kotlinc,
      ImmutableList<String> extraArguments,
      CompilerOutputPaths outputPaths,
      RelPath configuredBuckOut,
      KotlinCDAnalytics kotlinCDAnalytics,
      LanguageVersion languageVersion) {
    super(
        invokingRule,
        outputDirectory,
        sourceFilePaths,
        pathToSrcsList,
        combinedClassPathEntries,
        kotlinHomeLibraries,
        reportDirPath,
        kotlinc,
        extraArguments,
        ImmutableList.of(VERBOSE),
        outputPaths,
        false,
        configuredBuckOut,
        ImmutableMap.of(),
        null,
        ImmutableList.of(),
        ImmutableList.of(),
        false,
        ImmutableList.of(),
        Optional.empty(),
        KotlincMode.NonIncremental.INSTANCE,
        kotlinCDAnalytics,
        languageVersion,
        // Flag turning on/off K2 support for jvm-abi-gen actions
        // not part of KAPT, since jvm-abi-gen doesn't run on KAPT steps
        false,
        // Flag turning on/off shallow class usage for jvm-abi-gen dep files
        // not part of KAPT, since jvm-abi-gen doesn't run on KAPT steps
        false,
        // ABI validation mode for jvm-abi-gen repairs
        // not part of KAPT, since jvm-abi-gen doesn't run on KAPT steps
        "off",
        // jvm-abi-gen kill switch options
        // not part of KAPT, since jvm-abi-gen doesn't run on KAPT steps
        ImmutableList.of());
  }

  @Override
  protected boolean emitsClassToSourceMap() {
    return false;
  }

  @Override
  public String getShortName() {
    return "kapt";
  }
}
