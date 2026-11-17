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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;
import static org.mockito.Mockito.mock;
import static org.mockito.Mockito.when;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.core.filesystems.RelPath;
import com.facebook.buck.jvm.cd.command.kotlin.KotlinExtraParams;
import com.facebook.buck.jvm.java.ActionMetadata;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;

public class KotlincModeFactoryTest {

  private KotlinExtraParams mockKotlinExtraParams;
  private ActionMetadata mockActionMetadata;
  private IncrementalCompilationValidator incrementalCompilationValidator;
  private AbsPath absPath;
  private RelPath relPath;

  @Rule public TemporaryPaths temporaryPaths = new TemporaryPaths();

  @Before
  public void setUp() {
    mockKotlinExtraParams = mock(KotlinExtraParams.class);
    mockActionMetadata = mock(ActionMetadata.class);
    incrementalCompilationValidator = mock(IncrementalCompilationValidator.class);
    absPath = AbsPath.get("/");
    relPath = RelPath.get("");

    when(mockActionMetadata.getPreviousDigest()).thenReturn(new HashMap<>());
    when(mockActionMetadata.getCurrentDigest()).thenReturn(new HashMap<>());
    when(mockKotlinExtraParams.getKotlinCompilerPlugins()).thenReturn(ImmutableMap.of());
    when(incrementalCompilationValidator.validate(
            ImmutableList.of(), mockActionMetadata, absPath, absPath, absPath))
        .thenReturn(null);
  }

  @Test
  public void when_sourceOnly_then_nonIncremental() {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);

    KotlincMode kotlincMode =
        new KotlincModeFactory(incrementalCompilationValidator)
            .create(
                true,
                absPath,
                absPath,
                true,
                relPath,
                relPath,
                mockKotlinExtraParams,
                Optional.empty(),
                ImmutableList.of());

    assertTrue(kotlincMode instanceof KotlincMode.NonIncremental);
  }

  @Test
  public void when_not_shouldKotlincRunIncrementally_then_nonIncremental() {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(false);

    KotlincMode kotlincMode =
        new KotlincModeFactory(incrementalCompilationValidator)
            .create(
                false,
                absPath,
                absPath,
                true,
                relPath,
                relPath,
                mockKotlinExtraParams,
                Optional.empty(),
                ImmutableList.of());

    assertTrue(kotlincMode instanceof KotlincMode.NonIncremental);
  }

  @Test(expected = IllegalStateException.class)
  public void when_incrementalStateDir_empty_then_error() {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getIncrementalStateDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.empty());

    new KotlincModeFactory(incrementalCompilationValidator)
        .create(
            false,
            absPath,
            absPath,
            true,
            relPath,
            relPath,
            mockKotlinExtraParams,
            Optional.of(mockActionMetadata),
            ImmutableList.of());
  }

  @Test(expected = IllegalStateException.class)
  public void when_metadata_null_then_error() {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getIncrementalStateDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.empty());

    new KotlincModeFactory(incrementalCompilationValidator)
        .create(
            false,
            absPath,
            absPath,
            true,
            relPath,
            relPath,
            mockKotlinExtraParams,
            Optional.empty(),
            ImmutableList.of());
  }

  @Test
  public void
      when_shouldKotlincRunIncrementally_and_incrementalStateDir_isPresent_then_incremental() {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getIncrementalStateDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getJvmAbiGenWorkingDir()).thenReturn(Optional.of(AbsPath.get("/")));

    KotlincMode kotlincMode =
        new KotlincModeFactory(incrementalCompilationValidator)
            .create(
                false,
                absPath,
                absPath,
                true,
                relPath,
                relPath,
                mockKotlinExtraParams,
                Optional.of(mockActionMetadata),
                ImmutableList.of());

    assertTrue(kotlincMode instanceof KotlincMode.Incremental);
  }

  @Test
  public void incremental_mode_tracks_inputs_only_when_asked() {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getIncrementalStateDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockActionMetadata.getCurrentDigest())
        .thenReturn(ImmutableMap.of(Paths.get("plugin.jar"), "digest"));
    when(mockKotlinExtraParams.getExtraKotlincArguments())
        .thenReturn(ImmutableList.of("@args.txt"));
    when(mockKotlinExtraParams.getKotlinCompilerPlugins())
        .thenReturn(
            ImmutableMap.of(AbsPath.get("/plugin.jar"), ImmutableMap.of("config", "config.txt")));
    // Dependencies' source-only ABI jars: their content changes with every ABI change.
    when(mockKotlinExtraParams.getApplicabilityClasspath())
        .thenReturn(ImmutableList.of(AbsPath.get("/reduced.jar")));

    when(mockKotlinExtraParams.getShouldTrackIncrementalInputs()).thenReturn(true);
    KotlincMode.Incremental tracking =
        (KotlincMode.Incremental)
            new KotlincModeFactory(incrementalCompilationValidator)
                .create(
                    false,
                    absPath,
                    absPath,
                    false,
                    relPath,
                    relPath,
                    mockKotlinExtraParams,
                    Optional.of(mockActionMetadata),
                    ImmutableList.of());
    assertEquals(
        ImmutableMap.of(Paths.get("plugin.jar"), "digest"),
        Objects.requireNonNull(tracking.getInputTracking()).getInputDigests());
    assertEquals(
        ImmutableList.of("@args.txt", "config.txt"), tracking.getInputTracking().getOptionValues());

    when(mockKotlinExtraParams.getShouldTrackIncrementalInputs()).thenReturn(false);
    KotlincMode.Incremental notTracking =
        (KotlincMode.Incremental)
            new KotlincModeFactory(incrementalCompilationValidator)
                .create(
                    false,
                    absPath,
                    absPath,
                    false,
                    relPath,
                    relPath,
                    mockKotlinExtraParams,
                    Optional.of(mockActionMetadata),
                    ImmutableList.of());
    assertNull(notTracking.getInputTracking());
  }

  @Test
  public void when_is_track_class_usage_disabled_then_does_not_require_rebuild()
      throws IOException {
    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getIncrementalStateDir())
        .thenReturn(Optional.of(temporaryPaths.newFolder("incrementalStateDir")));
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.of(AbsPath.get("/")));

    KotlincMode kotlincMode =
        new KotlincModeFactory(incrementalCompilationValidator)
            .create(
                false,
                absPath,
                absPath,
                false,
                relPath,
                relPath,
                mockKotlinExtraParams,
                Optional.of(mockActionMetadata),
                ImmutableList.of());

    assertTrue(kotlincMode instanceof KotlincMode.Incremental);
    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_depTrackerPlugin_changed_then_rebuild() throws IOException {
    KotlincMode kotlincMode =
        createIncrementalModeWithRealValidator(
            true, "old-dep-tracker-digest", "new-dep-tracker-digest");

    assertTrue(kotlincMode instanceof KotlincMode.Incremental);
    assertEquals(
        RebuildReason.KOTLIN_COMPILER_PLUGIN_CHANGED,
        ((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_depTrackerPlugin_unchanged_then_no_rebuild() throws IOException {
    KotlincMode kotlincMode =
        createIncrementalModeWithRealValidator(
            true, "same-dep-tracker-digest", "same-dep-tracker-digest");

    assertTrue(kotlincMode instanceof KotlincMode.Incremental);
    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_depTrackerPlugin_changed_but_classUsageTracking_disabled_then_no_rebuild()
      throws IOException {
    KotlincMode kotlincMode =
        createIncrementalModeWithRealValidator(
            false, "old-dep-tracker-digest", "new-dep-tracker-digest");

    assertTrue(kotlincMode instanceof KotlincMode.Incremental);
    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  private KotlincMode createIncrementalModeWithRealValidator(
      boolean trackClassUsage, String previousPluginDigest, String currentPluginDigest)
      throws IOException {
    AbsPath root = temporaryPaths.getRoot();
    AbsPath depTracker = root.resolve("dep-tracker.jar");
    if (trackClassUsage) {
      temporaryPaths.newFile("dep-file.txt");
      temporaryPaths.newFile("used-jars.json");
    }

    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getShouldUseJvmAbiGen()).thenReturn(false);
    when(mockKotlinExtraParams.getDepTrackerPlugin()).thenReturn(Optional.of(depTracker));
    when(mockActionMetadata.getPreviousIncrementalConfigDigest()).thenReturn("config");
    when(mockActionMetadata.getCurrentIncrementalConfigDigest()).thenReturn("config");
    Path jarKey = root.relativize(depTracker).getPath();
    Map<Path, String> previousDigest = new HashMap<>();
    previousDigest.put(jarKey, previousPluginDigest);
    Map<Path, String> currentDigest = new HashMap<>();
    currentDigest.put(jarKey, currentPluginDigest);
    when(mockActionMetadata.getPreviousDigest()).thenReturn(previousDigest);
    when(mockActionMetadata.getCurrentDigest()).thenReturn(currentDigest);

    return new KotlincModeFactory(new IncrementalCompilationValidator())
        .create(
            false,
            root,
            AbsPath.get("/"),
            trackClassUsage,
            RelPath.get("dep-file.txt"),
            RelPath.get("used-jars.json"),
            mockKotlinExtraParams,
            Optional.of(mockActionMetadata),
            ImmutableList.of());
  }
}
