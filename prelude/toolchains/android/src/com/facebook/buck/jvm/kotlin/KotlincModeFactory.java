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
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.IncrementalInputTracking;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import java.nio.file.Path;
import java.util.Optional;
import org.jspecify.annotations.Nullable;

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
      @Nullable AbsPath depFile =
          isTrackClassUsageEnabled ? rootProjectDir.resolve(depFilePath) : null;
      @Nullable AbsPath usedJars =
          isTrackClassUsageEnabled ? rootProjectDir.resolve(usedJarsPath) : null;

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
      // So does the Kosabi applicability plugin, whose rules decide the verdict of every round.
      boolean verifiesApplicability = extraParams.getShouldVerifySourceOnlyAbiConstraints();
      @Nullable RelPath applicabilityPlugin =
          verifiesApplicability
              ? Optional.ofNullable(
                      extraParams
                          .getKosabiPluginOptions()
                          .get(KosabiConfig.PROPERTY_KOSABI_APPLICABILITY_PLUGIN))
                  .map(rootProjectDir::relativize)
                  .orElse(null)
              : null;
      if (applicabilityPlugin != null) {
        watchedPlugins.add(applicabilityPlugin);
      }

      @Nullable RebuildReason rebuildReason =
          incrementalCompilationValidator.validate(
              watchedPlugins.build(),
              metadata,
              depFile,
              usedJars,
              getJvmAbiGenWorkingDir(
                  extraParams.getShouldUseJvmAbiGen(), extraParams.getJvmAbiGenWorkingDir()));
      // Recorded on every round, so that a successful one leaves the state the next compares with.
      if (verifiesApplicability
          && hasApplicabilityClasspathChanged(rootProjectDir, extraParams, metadata)
          && rebuildReason == null) {
        rebuildReason = RebuildReason.APPLICABILITY_CLASSPATH_CHANGED;
      }
      // The plugin watch compares digests; without one for the plugin it cannot see a change.
      if (applicabilityPlugin != null
          && !metadata.getCurrentDigest().containsKey(applicabilityPlugin.getPath())
          && rebuildReason == null) {
        rebuildReason = RebuildReason.KOTLIN_COMPILER_PLUGIN_CHANGED;
      }

      if (verifiesApplicability) {
        rebuildReason = startApplicabilityRound(extraParams, rebuildReason);
      }

      return new KotlincMode.Incremental(
          rootProjectDir,
          buildDir,
          kotlicWorkingDir,
          KotlinSourceChanges.ToBeCalculated.INSTANCE,
          ClasspathChangesFactory.create(new SnapshotsActionMetadata(metadata), classpathSnapshots),
          depFile,
          rebuildReason,
          extraParams.getShouldTrackIncrementalInputs()
              ? new IncrementalInputTracking(metadata.getCurrentDigest(), optionValues(extraParams))
              : null);
    }
  }

  /**
   * Compiles in full after a round that did not finish (see {@link ApplicabilityRoundMarker}), and
   * records that this one started.
   */
  private static @Nullable RebuildReason startApplicabilityRound(
      KotlinExtraParams extraParams, @Nullable RebuildReason rebuildReason) {
    Path incrementalStateDir =
        extraParams
            .getIncrementalStateDir()
            .orElseThrow(() -> new IllegalStateException("incremental_state_dir is not created"))
            .getPath();
    @Nullable RebuildReason result = rebuildReason;
    boolean unfinished = ApplicabilityRoundMarker.lastRoundDidNotFinish(incrementalStateDir);
    if (unfinished && result == null) {
      result = RebuildReason.APPLICABILITY_ROUND_DID_NOT_FINISH;
    }
    if (unfinished && ApplicabilityRoundMarker.isRecorded(incrementalStateDir)) {
      // The unfinished round stays recorded until this round's compile moves the marker on: a
      // round that fails before it compiles (in an annotation processor, for one) must not make
      // the next round incremental.
      return result;
    }
    try {
      ApplicabilityRoundMarker.enter(incrementalStateDir, ApplicabilityRoundMarker.Phase.STARTED);
    } catch (IOException e) {
      LOG.warn(e, "Could not mark the Kosabi applicability round; compiling in full");
      if (result == null) {
        result = RebuildReason.APPLICABILITY_ROUND_DID_NOT_FINISH;
      }
    }
    return result;
  }

  private static boolean hasApplicabilityClasspathChanged(
      AbsPath rootProjectDir, KotlinExtraParams extraParams, ActionMetadata metadata) {
    AbsPath incrementalStateDir =
        extraParams
            .getIncrementalStateDir()
            .orElseThrow(() -> new IllegalStateException("incremental_state_dir is not created"));
    try {
      return ApplicabilityClasspathState.recordAndCompare(
          rootProjectDir, incrementalStateDir, extraParams.getApplicabilityClasspath(), metadata);
    } catch (IOException e) {
      LOG.warn(e, "Could not compare the Kosabi applicability classpath; compiling in full");
      return true;
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

  /**
   * The target's own compiler arguments and compiler plugin option values: the values that may name
   * files whose content changes the output at the same path. The reduced classpath of the Kosabi
   * applicability plugin is not among them: those jars are the dependencies' source-only ABI jars,
   * so their content changes with every ABI change of a dependency.
   */
  private static ImmutableList<String> optionValues(KotlinExtraParams extraParams) {
    ImmutableList.Builder<String> values = ImmutableList.builder();
    values.addAll(extraParams.getExtraKotlincArguments());
    for (ImmutableMap<String, String> options : extraParams.getKotlinCompilerPlugins().values()) {
      values.addAll(options.values());
    }
    return values.build();
  }
}
