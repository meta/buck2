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
import static org.junit.Assert.assertFalse;
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
import java.io.OutputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.nio.file.attribute.PosixFilePermissions;
import java.util.HashMap;
import java.util.Map;
import java.util.Objects;
import java.util.Optional;
import java.util.jar.JarEntry;
import java.util.jar.JarOutputStream;
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

  @Test
  public void when_applicabilityPlugin_changed_then_rebuild() throws IOException {
    KotlincMode kotlincMode =
        createApplicabilityMode(true, "old-applicability-digest", "new-applicability-digest");

    assertEquals(
        RebuildReason.KOTLIN_COMPILER_PLUGIN_CHANGED,
        ((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_applicabilityPlugin_changed_but_applicability_off_then_no_rebuild()
      throws IOException {
    KotlincMode kotlincMode =
        createApplicabilityMode(false, "old-applicability-digest", "new-applicability-digest");

    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_applicabilityClasspath_changed_then_rebuild() throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    KotlincMode first = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    // The first round has nothing to compare with, so it is full.
    assertEquals(
        RebuildReason.APPLICABILITY_CLASSPATH_CHANGED,
        ((KotlincMode.Incremental) first).getRebuildReason());
    succeedRound(stateDir);

    KotlincMode unchanged = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    assertNull(((KotlincMode.Incremental) unchanged).getRebuildReason());
    succeedRound(stateDir);

    writeJar(reducedJar, "p/A.class", "p/B.class");
    KotlincMode changed = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    assertEquals(
        RebuildReason.APPLICABILITY_CLASSPATH_CHANGED,
        ((KotlincMode.Incremental) changed).getRebuildReason());
  }

  @Test
  public void when_applicabilityPlugin_has_no_digest_then_rebuild() throws IOException {
    KotlincMode kotlincMode = createApplicabilityMode(true, "same", "same");
    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());

    // Both rounds without a digest for the plugin: the watch alone would call it unchanged.
    AbsPath stateDir = temporaryPaths.newFolder("no_digest_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    succeedRound(stateDir);
    when(mockActionMetadata.getPreviousDigest()).thenReturn(new HashMap<>());
    when(mockActionMetadata.getCurrentDigest()).thenReturn(new HashMap<>());
    KotlincMode withoutDigest =
        new KotlincModeFactory(new IncrementalCompilationValidator())
            .create(
                false,
                temporaryPaths.getRoot(),
                AbsPath.get("/"),
                false,
                RelPath.get("dep-file.txt"),
                RelPath.get("used-jars.json"),
                mockKotlinExtraParams,
                Optional.of(mockActionMetadata),
                ImmutableList.of());

    assertEquals(
        RebuildReason.KOTLIN_COMPILER_PLUGIN_CHANGED,
        ((KotlincMode.Incremental) withoutDigest).getRebuildReason());
  }

  @Test
  public void when_last_applicability_round_did_not_finish_then_rebuild() throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    succeedRound(stateDir);

    // A clean compile failure leaves the round marked as started: still incremental.
    ApplicabilityRoundMarker.enter(stateDir.getPath(), ApplicabilityRoundMarker.Phase.STARTED);
    assertNull(
        ((KotlincMode.Incremental)
                createApplicabilityMode(true, "same", "same", stateDir, reducedJar))
            .getRebuildReason());

    // A compile that was committed before the action failed.
    ApplicabilityRoundMarker.enter(stateDir.getPath(), ApplicabilityRoundMarker.Phase.COMPILED);
    KotlincMode next = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);

    assertEquals(
        RebuildReason.APPLICABILITY_ROUND_DID_NOT_FINISH,
        ((KotlincMode.Incremental) next).getRebuildReason());
    // That round fails before it compiles: the round after it is full too.
    KotlincMode afterThat = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    assertEquals(
        RebuildReason.APPLICABILITY_ROUND_DID_NOT_FINISH,
        ((KotlincMode.Incremental) afterThat).getRebuildReason());
    assertTrue(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir.getPath()));
  }

  @Test
  public void when_applicabilityLedger_missing_then_rebuild() throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    // A successful round whose plugin left no ledger.
    ApplicabilityClasspathState.promotePending(stateDir.getPath());

    KotlincMode next = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);

    assertEquals(
        RebuildReason.APPLICABILITY_LEDGER_NOT_FOUND,
        ((KotlincMode.Incremental) next).getRebuildReason());
  }

  @Test
  public void when_applicabilityLedger_unreadable_then_rebuild() throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    succeedRound(stateDir);
    Path committed = stateDir.resolve(ApplicabilityLedgerFiles.COMMITTED_FILE).getPath();
    Files.setPosixFilePermissions(committed, PosixFilePermissions.fromString("---------"));
    try {
      KotlincMode next = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);

      assertEquals(
          RebuildReason.APPLICABILITY_LEDGER_NOT_FOUND,
          ((KotlincMode.Incremental) next).getRebuildReason());
    } finally {
      Files.setPosixFilePermissions(committed, PosixFilePermissions.fromString("rw-r--r--"));
    }
  }

  @Test
  public void when_round_starts_then_stale_pending_ledger_is_discarded() throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    createApplicabilityMode(true, "same", "same", stateDir, reducedJar);
    succeedRound(stateDir);
    // A failed round's ledger, which a later success must not promote.
    Path stalePending = stateDir.resolve(ApplicabilityLedgerFiles.PENDING_FILE).getPath();
    Files.writeString(stalePending, "stale");

    KotlincMode next = createApplicabilityMode(true, "same", "same", stateDir, reducedJar);

    assertNull(((KotlincMode.Incremental) next).getRebuildReason());
    assertFalse(Files.exists(stalePending));
  }

  @Test
  public void when_applicabilityJavaSources_changed_then_rebuild() throws IOException {
    Path java = Paths.get("src/Bindings.java");
    KotlincMode kotlincMode =
        createApplicabilityModeWithJava(
            true, ImmutableMap.of(java, "old"), ImmutableMap.of(java, "new"));

    assertEquals(
        RebuildReason.APPLICABILITY_JAVA_SOURCES_CHANGED,
        ((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_applicabilityJavaSource_removed_then_rebuild() throws IOException {
    Path java = Paths.get("src/Bindings.java");
    KotlincMode kotlincMode =
        createApplicabilityModeWithJava(true, ImmutableMap.of(java, "old"), ImmutableMap.of());

    assertEquals(
        RebuildReason.APPLICABILITY_JAVA_SOURCES_CHANGED,
        ((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_applicabilityJavaSources_unchanged_then_no_rebuild() throws IOException {
    Path java = Paths.get("src/Bindings.java");
    KotlincMode kotlincMode =
        createApplicabilityModeWithJava(
            true, ImmutableMap.of(java, "same"), ImmutableMap.of(java, "same"));

    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  @Test
  public void when_applicabilityJavaSources_changed_but_applicability_off_then_no_rebuild()
      throws IOException {
    Path java = Paths.get("src/Bindings.java");
    KotlincMode kotlincMode =
        createApplicabilityModeWithJava(
            false, ImmutableMap.of(java, "old"), ImmutableMap.of(java, "new"));

    assertNull(((KotlincMode.Incremental) kotlincMode).getRebuildReason());
  }

  private KotlincMode createApplicabilityModeWithJava(
      boolean verifiesApplicability, Map<Path, String> previousJava, Map<Path, String> currentJava)
      throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    // A round with a recorded classpath, so only the Java sources can force a full one.
    createApplicabilityMode(
        verifiesApplicability, "same", "same", stateDir, reducedJar, previousJava, previousJava);
    succeedRound(stateDir);
    return createApplicabilityMode(
        verifiesApplicability, "same", "same", stateDir, reducedJar, previousJava, currentJava);
  }

  /** What a successful round leaves: the plugin's pending ledger, then kotlincd's promotion. */
  private static void succeedRound(AbsPath stateDir) throws IOException {
    Files.writeString(stateDir.resolve(ApplicabilityLedgerFiles.PENDING_FILE).getPath(), "ledger");
    ApplicabilityClasspathState.promotePending(stateDir.getPath());
    ApplicabilityLedgerFiles.promotePending(stateDir.getPath());
    ApplicabilityRoundMarker.clear(stateDir.getPath());
  }

  private KotlincMode createApplicabilityMode(
      boolean verifiesApplicability, String previousPluginDigest, String currentPluginDigest)
      throws IOException {
    AbsPath stateDir = temporaryPaths.newFolder("incremental_state");
    AbsPath reducedJar = temporaryPaths.getRoot().resolve("reduced.jar");
    writeJar(reducedJar, "p/A.class");
    // A round with a recorded classpath, so only the plugin can force a full one.
    createApplicabilityMode(
        verifiesApplicability, previousPluginDigest, previousPluginDigest, stateDir, reducedJar);
    succeedRound(stateDir);
    return createApplicabilityMode(
        verifiesApplicability, previousPluginDigest, currentPluginDigest, stateDir, reducedJar);
  }

  private KotlincMode createApplicabilityMode(
      boolean verifiesApplicability,
      String previousPluginDigest,
      String currentPluginDigest,
      AbsPath stateDir,
      AbsPath reducedJar) {
    return createApplicabilityMode(
        verifiesApplicability,
        previousPluginDigest,
        currentPluginDigest,
        stateDir,
        reducedJar,
        ImmutableMap.of(),
        ImmutableMap.of());
  }

  private KotlincMode createApplicabilityMode(
      boolean verifiesApplicability,
      String previousPluginDigest,
      String currentPluginDigest,
      AbsPath stateDir,
      AbsPath reducedJar,
      Map<Path, String> previousExtra,
      Map<Path, String> currentExtra) {
    AbsPath root = temporaryPaths.getRoot();
    AbsPath plugin = root.resolve("applicability.jar");

    when(mockKotlinExtraParams.getShouldKotlincRunIncrementally()).thenReturn(true);
    when(mockKotlinExtraParams.getKotlincWorkingDir()).thenReturn(Optional.of(AbsPath.get("/")));
    when(mockKotlinExtraParams.getIncrementalStateDir()).thenReturn(Optional.of(stateDir));
    when(mockKotlinExtraParams.getShouldUseJvmAbiGen()).thenReturn(false);
    when(mockKotlinExtraParams.getDepTrackerPlugin()).thenReturn(Optional.empty());
    when(mockKotlinExtraParams.getShouldVerifySourceOnlyAbiConstraints())
        .thenReturn(verifiesApplicability);
    when(mockKotlinExtraParams.getKosabiPluginOptions())
        .thenReturn(ImmutableMap.of(KosabiConfig.PROPERTY_KOSABI_APPLICABILITY_PLUGIN, plugin));
    when(mockKotlinExtraParams.getApplicabilityClasspath())
        .thenReturn(ImmutableList.of(reducedJar));
    when(mockActionMetadata.getPreviousIncrementalConfigDigest()).thenReturn("config");
    when(mockActionMetadata.getCurrentIncrementalConfigDigest()).thenReturn("config");
    Path pluginKey = root.relativize(plugin).getPath();
    Map<Path, String> previousDigest = new HashMap<>(previousExtra);
    previousDigest.put(pluginKey, previousPluginDigest);
    Map<Path, String> currentDigest = new HashMap<>(currentExtra);
    currentDigest.put(pluginKey, currentPluginDigest);
    when(mockActionMetadata.getPreviousDigest()).thenReturn(previousDigest);
    when(mockActionMetadata.getCurrentDigest()).thenReturn(currentDigest);

    return new KotlincModeFactory(new IncrementalCompilationValidator())
        .create(
            false,
            root,
            AbsPath.get("/"),
            false,
            RelPath.get("dep-file.txt"),
            RelPath.get("used-jars.json"),
            mockKotlinExtraParams,
            Optional.of(mockActionMetadata),
            ImmutableList.of());
  }

  private static void writeJar(AbsPath jar, String... classEntries) throws IOException {
    try (OutputStream file = Files.newOutputStream(jar.getPath());
        JarOutputStream out = new JarOutputStream(file)) {
      for (String entry : classEntries) {
        out.putNextEntry(new JarEntry(entry));
        out.closeEntry();
      }
    }
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
