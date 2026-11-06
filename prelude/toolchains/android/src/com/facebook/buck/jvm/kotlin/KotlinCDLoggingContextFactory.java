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
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion;
import com.facebook.buck.jvm.kotlin.cd.analytics.ClasspathChangesParam;
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext;
import com.facebook.buck.jvm.kotlin.cd.analytics.ModeParam;
import com.facebook.buck.jvm.kotlin.cd.analytics.StepParam;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.ClasspathChanges;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.infer.annotation.Nullsafe;
import java.util.Set;
import java.util.stream.Collectors;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlinCDLoggingContextFactory {

  private KotlinCDLoggingContextFactory() {}

  public static KotlinCDLoggingContext create(
      KotlincStep kotlincStep, LanguageVersion languageVersion, KotlincMode kotlincMode) {
    return new KotlinCDLoggingContext(create(kotlincStep), languageVersion, create(kotlincMode));
  }

  private static ModeParam create(KotlincMode kotlincMode) {
    if (kotlincMode instanceof KotlincMode.NonIncremental) {
      return ModeParam.NonIncremental.INSTANCE;
    } else if (kotlincMode instanceof KotlincMode.Incremental) {
      KotlincMode.Incremental incrementalMode = (KotlincMode.Incremental) kotlincMode;
      Set<AbsPath> removedFiles = null;
      Set<AbsPath> modifiedFiles = null;

      if (incrementalMode.getKotlinSourceChanges() instanceof KotlinSourceChanges.Known) {
        KotlinSourceChanges.Known knownSourceChanges =
            (KotlinSourceChanges.Known) incrementalMode.getKotlinSourceChanges();

        modifiedFiles =
            knownSourceChanges.getAddedAndModifiedFiles().stream().collect(Collectors.toSet());
        removedFiles = knownSourceChanges.getRemovedFiles().stream().collect(Collectors.toSet());
      }

      return new ModeParam.Incremental(
          create(incrementalMode.getClasspathChanges()), modifiedFiles, removedFiles);
    } else {
      throw new IllegalArgumentException("Unsupported kotlinc mode: " + kotlincMode);
    }
  }

  private static ClasspathChangesParam create(ClasspathChanges classpathChanges) {
    if (classpathChanges instanceof ClasspathChanges.ToBeComputedByIncrementalCompiler) {
      return ClasspathChangesParam.TO_BE_COMPUTED_BY_INCREMENTAL_COMPILER;
    } else if (classpathChanges instanceof ClasspathChanges.NoChanges) {
      return ClasspathChangesParam.NO_CHANGES;
    } else if (classpathChanges instanceof ClasspathChanges.Unknown) {
      return ClasspathChangesParam.UNKNOWN;
    } else {
      throw new IllegalArgumentException("Unsupported classpathChanges mode: " + classpathChanges);
    }
  }

  private static StepParam create(KotlincStep kotlincStep) {
    // KSP2 step is emitted by its own factory
    // TODO asoshin: move all other steps to use KSP2 pattern for logging
    if (kotlincStep instanceof KosabiStubgenStep) {
      return StepParam.KOSABI_STUBGEN;
    } else if (kotlincStep instanceof KaptStep) {
      return StepParam.KAPT;
    } else {
      return StepParam.KOTLINC;
    }
  }
}
