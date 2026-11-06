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
import com.facebook.buck.core.util.log.Logger;
import com.facebook.buck.jvm.cd.command.kotlin.KotlinExtraParams;
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import java.util.Optional;
import javax.annotation.Nullable;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class KotlincModeFactory {
  private static final Logger LOG = Logger.get(KotlincModeFactory.class);

  private final IncrementalCompilationValidator incrementalCompilationValidator;

  public KotlincModeFactory() {
    this(new IncrementalCompilationValidator());
  }

  public KotlincModeFactory(IncrementalCompilationValidator incrementalCompilationValidator) {
    this.incrementalCompilationValidator = incrementalCompilationValidator;
  }

  public KotlincMode create(
      boolean isSourceOnly,
      final AbsPath rootProjectDir,
      final AbsPath buildDir,
      final boolean isTrackClassUsageEnabled,
      final RelPath depFilePath,
      final RelPath usedJarsPath,
      final KotlinExtraParams extraParams,
      final Optional<ActionMetadata> actionMetadata,
      final ImmutableList<AbsPath> classpathSnapshots) {
    if (!extraParams.getShouldKotlincRunIncrementally()) {
      LOG.info("Non-incremental mode applied: incremental property disabled");
      return KotlincMode.NonIncremental.INSTANCE;
    } else if (isSourceOnly) {
      LOG.info("Non-incremental mode applied: source-only build requested");
      return KotlincMode.NonIncremental.INSTANCE;
    } else {
      @Nullable
      AbsPath depFile = isTrackClassUsageEnabled ? rootProjectDir.resolve(depFilePath) : null;
      @Nullable
      AbsPath usedJars = isTrackClassUsageEnabled ? rootProjectDir.resolve(usedJarsPath) : null;

      AbsPath kotlicWorkingDir =
          extraParams
              .getKotlincWorkingDir()
              .orElseThrow(
                  () ->
                      new IllegalStateException(
                          "incremental_state_dir/kotlinc_working_dir is not created"));
      ActionMetadata metadata =
          actionMetadata.orElseThrow(
              () -> new IllegalStateException("actionMetadata is not created"));
      LOG.info("Incremental mode applied");

      ImmutableList.Builder<RelPath> watchedPlugins = ImmutableList.builder();
      extraParams.getKotlinCompilerPlugins().keySet().stream()
          .map(rootProjectDir::relativize)
          .forEach(watchedPlugins::add);
      // The dep-tracker plugin rides outside the compiler-plugins map but still shapes
      // outputs (dep files); watch it too, exactly when kotlinc will load it.
      if (isTrackClassUsageEnabled) {
        extraParams
            .getDepTrackerPlugin()
            .map(rootProjectDir::relativize)
            .ifPresent(watchedPlugins::add);
      }

      return new KotlincMode.Incremental(
          rootProjectDir,
          buildDir,
          kotlicWorkingDir,
          KotlinSourceChanges.ToBeCalculated.INSTANCE,
          ClasspathChangesFactory.create(new SnapshotsActionMetadata(metadata), classpathSnapshots),
          depFile,
          incrementalCompilationValidator.validate(
              watchedPlugins.build(),
              metadata,
              depFile,
              usedJars,
              getJvmAbiGenWorkingDir(
                  extraParams.getShouldUseJvmAbiGen(), extraParams.getJvmAbiGenWorkingDir())));
    }
  }

  private static @Nullable AbsPath getJvmAbiGenWorkingDir(
      boolean shouldUseJvmAbiGen, Optional<AbsPath> jvmAbiGenWorkingDir) {
    if (!shouldUseJvmAbiGen) {
      return null;
    }

    return jvmAbiGenWorkingDir.orElseThrow(
        () -> new IllegalStateException("jvm_abi_gen_working_dir is not created"));
  }
}
