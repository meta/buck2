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
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.core.filesystems.AbsPath;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.ClasspathChanges;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode;
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason;
import com.facebook.buck.testutil.TemporaryPaths;
import com.google.common.collect.ImmutableList;
import com.google.common.collect.ImmutableMap;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.Optional;
import java.util.function.Supplier;
import javax.annotation.Nullable;
import org.junit.Rule;
import org.junit.Test;

public class KotlincStepTest {
  private static final AbsPath TEST_ROOT = AbsPath.of(Paths.get(".").toAbsolutePath().normalize());

  @Rule public TemporaryPaths temporaryPaths = new TemporaryPaths();

  @Test
  public void structuredOptionsIncludeContextAndExplicitEmptyClasspath() {
    ImmutableList<String> options =
        KotlincStep.getKosabiApplicabilityPluginOptions(
            "fbcode//example:target", TEST_ROOT, ImmutableList.of(), Optional.of(path("fbcode")));

    assertEquals(
        ImmutableList.of(
            "-P",
            "plugin:com.facebook.kotlin.compilerplugins.kosabiapplicability:target-label=fbcode//example:target",
            "-P",
            "plugin:com.facebook.kotlin.compilerplugins.kosabiapplicability:source-root="
                + TEST_ROOT.getPath(),
            "-P",
            "plugin:com.facebook.kotlin.compilerplugins.kosabiapplicability:source-root-prefix=fbcode",
            "-P",
            "plugin:com.facebook.kotlin.compilerplugins.kosabiapplicability:source-only-abi-classpath="),
        options);
  }

  @Test
  public void structuredOptionsFailClosedWithoutRequiredBuckContext() {
    IllegalStateException missingPlugin =
        assertThrows(
            IllegalStateException.class,
            () -> KotlincStep.getRequiredKosabiApplicabilityPlugin(ImmutableMap.of()));
    assertTrue(missingPlugin.getMessage().contains("plugin path is missing"));

    IllegalStateException missingCellRoot =
        assertThrows(
            IllegalStateException.class,
            () ->
                KotlincStep.getKosabiApplicabilityPluginOptions(
                    "fbcode//example:target", TEST_ROOT, ImmutableList.of(), Optional.empty()));
    assertTrue(missingCellRoot.getMessage().contains("cell root path is missing"));
  }

  @Test
  public void onlyAnIncrementalApplicabilityRoundCanBeCheckedAgainInFull() throws IOException {
    AbsPath workingDir = temporaryPaths.newFolder("kotlinc_working_dir");
    KotlincMode.Incremental incremental = incrementalMode(workingDir, null);

    // No successful earlier round: the incremental compiler compiles in full anyway.
    assertFalse(KotlincStep.isIncrementalApplicabilityRound(true, incremental));

    Files.writeString(workingDir.resolve("shrunk-classpath-snapshot.bin").getPath(), "");
    assertTrue(KotlincStep.isIncrementalApplicabilityRound(true, incremental));
    assertFalse(KotlincStep.isIncrementalApplicabilityRound(false, incremental));
    assertFalse(
        KotlincStep.isIncrementalApplicabilityRound(
            true, incrementalMode(workingDir, RebuildReason.KOTLIN_COMPILER_PLUGIN_CHANGED)));
    assertFalse(
        KotlincStep.isIncrementalApplicabilityRound(true, KotlincMode.NonIncremental.INSTANCE));
  }

  @Test
  public void onlyAnApplicabilityVerdictIsCheckedAgainInFull() {
    String report = "error: KOSABI_APPLICABILITY_REPORT_V2={\"status\":\"BLOCKED\"}";

    assertTrue(KotlincStep.shouldCheckAgainInFull(true, 1, () -> report));
    // Any other compiler error suppresses the report, and fails the round as it is.
    assertFalse(
        KotlincStep.shouldCheckAgainInFull(true, 1, () -> "error: unresolved reference 'x'"));
    // Other rounds do not even read the output.
    Supplier<String> unread =
        () -> {
          throw new AssertionError("The output was read");
        };
    assertFalse(KotlincStep.shouldCheckAgainInFull(true, 0, unread));
    assertFalse(KotlincStep.shouldCheckAgainInFull(false, 1, unread));
  }

  private static final String BLOCKED_REPORT =
      "error: KOSABI_APPLICABILITY_REPORT_V2={\"status\":\"BLOCKED\"}";

  /** Records each compile's mode and answers with the next exit code. */
  private static final class FakeCompilation implements KotlincStep.Compilation {
    final java.util.List<KotlincMode> modes = new java.util.ArrayList<>();
    private final java.util.Iterator<Integer> exitCodes;

    FakeCompilation(Integer... exitCodes) {
      this.exitCodes = java.util.Arrays.asList(exitCodes).iterator();
    }

    @Override
    public int compile(KotlincMode mode) {
      modes.add(mode);
      return exitCodes.next();
    }
  }

  /** Records whether the state copy was put back or dropped, failing both with [failure]. */
  private static final class FakeStateCopier implements KotlincStep.StateCopier {
    int copies;
    int restores;
    int discards;
    @Nullable IOException failure;

    @Override
    public KotlincStep.StateCopy copy() {
      copies++;
      return new KotlincStep.StateCopy() {
        @Override
        public void restore() throws IOException {
          restores++;
          if (failure != null) {
            throw failure;
          }
        }

        @Override
        public void discard() throws IOException {
          discards++;
          if (failure != null) {
            throw failure;
          }
        }
      };
    }
  }

  private int compileCheckingAgainInFull(
      KotlincMode mode,
      FakeCompilation compilation,
      boolean compiledIncrementally,
      FakeStateCopier copier)
      throws Exception {
    return KotlincStep.compileCheckingAgainInFull(
        true,
        mode,
        compilation,
        () -> compiledIncrementally,
        () -> BLOCKED_REPORT,
        () -> {},
        copier,
        "//test:target");
  }

  private int rounds;

  private KotlincMode.Incremental incrementalRound() throws IOException {
    AbsPath workingDir = temporaryPaths.newFolder("round" + rounds++, "kotlinc_working_dir");
    Files.writeString(workingDir.resolve("shrunk-classpath-snapshot.bin").getPath(), "");
    return incrementalMode(workingDir, null);
  }

  @Test
  public void aBlockedIncrementalRoundIsCompiledAgainInFullAndKeepsItsStateIfBlockedAgain()
      throws Exception {
    KotlincMode.Incremental mode = incrementalRound();
    FakeCompilation compilation = new FakeCompilation(1, 1);
    FakeStateCopier copier = new FakeStateCopier();

    assertEquals(1, compileCheckingAgainInFull(mode, compilation, true, copier));

    assertEquals(2, compilation.modes.size());
    assertEquals(
        RebuildReason.APPLICABILITY_CHECKED_IN_FULL,
        ((KotlincMode.Incremental) compilation.modes.get(1)).getRebuildReason());
    // The full compile failed too: the state of the last successful round is put back.
    assertEquals(1, copier.copies);
    assertEquals(1, copier.restores);
    assertEquals(0, copier.discards);
  }

  @Test
  public void aBlockedResultStandsWhenTheStateCannotBePutBack() throws Exception {
    FakeStateCopier copier = new FakeStateCopier();
    copier.failure = new IOException("disk full");

    assertEquals(
        1, compileCheckingAgainInFull(incrementalRound(), new FakeCompilation(1, 1), true, copier));
    assertEquals(1, copier.restores);
  }

  @Test
  public void aPassingResultStandsWhenTheCopyCannotBeDropped() throws Exception {
    FakeStateCopier copier = new FakeStateCopier();
    copier.failure = new IOException("disk full");

    assertEquals(
        0, compileCheckingAgainInFull(incrementalRound(), new FakeCompilation(1, 0), true, copier));
    assertEquals(1, copier.discards);
  }

  @Test
  public void aCompilerFailureIsNotHiddenByAStateThatCannotBePutBack() throws Exception {
    KotlincMode.Incremental mode = incrementalRound();
    FakeStateCopier copier = new FakeStateCopier();
    IOException restoreFailure = new IOException("disk full");
    copier.failure = restoreFailure;
    java.util.concurrent.atomic.AtomicInteger compiles =
        new java.util.concurrent.atomic.AtomicInteger();
    KotlincStep.Compilation compilation =
        compileMode -> {
          if (compiles.incrementAndGet() == 2) {
            throw new IllegalStateException("compiler crashed");
          }
          return 1;
        };

    IllegalStateException thrown =
        assertThrows(
            IllegalStateException.class,
            () ->
                KotlincStep.compileCheckingAgainInFull(
                    true,
                    mode,
                    compilation,
                    () -> true,
                    () -> BLOCKED_REPORT,
                    () -> {},
                    copier,
                    "//test:target"));

    assertEquals(restoreFailure, thrown.getSuppressed()[0]);
  }

  @Test
  public void aFullRoundThatPassesKeepsItsOwnState() throws Exception {
    FakeCompilation compilation = new FakeCompilation(1, 0);
    FakeStateCopier copier = new FakeStateCopier();

    assertEquals(0, compileCheckingAgainInFull(incrementalRound(), compilation, true, copier));

    assertEquals(2, compilation.modes.size());
    assertEquals(0, copier.restores);
    assertEquals(1, copier.discards);
  }

  @Test
  public void aRoundThatAlreadyCompiledInFullIsNotCompiledAgain() throws Exception {
    // An incremental mode in which the compiler compiled in full (changed arguments, for one).
    FakeCompilation compilation = new FakeCompilation(1);
    FakeStateCopier copier = new FakeStateCopier();

    assertEquals(1, compileCheckingAgainInFull(incrementalRound(), compilation, false, copier));

    assertEquals(1, compilation.modes.size());
    assertEquals(0, copier.copies);
  }

  @Test
  public void aPassingOrNonApplicabilityRoundIsCompiledOnce() throws Exception {
    FakeCompilation passing = new FakeCompilation(0);
    assertEquals(
        0, compileCheckingAgainInFull(incrementalRound(), passing, true, new FakeStateCopier()));
    assertEquals(1, passing.modes.size());

    FakeCompilation notVerifying = new FakeCompilation(1);
    assertEquals(
        1,
        KotlincStep.compileCheckingAgainInFull(
            false,
            incrementalRound(),
            notVerifying,
            () -> true,
            () -> BLOCKED_REPORT,
            () -> {},
            new FakeStateCopier(),
            "//test:target"));
    assertEquals(1, notVerifying.modes.size());
  }

  private static KotlincMode.Incremental incrementalMode(
      AbsPath workingDir, @Nullable RebuildReason rebuildReason) {
    return new KotlincMode.Incremental(
        TEST_ROOT,
        TEST_ROOT,
        workingDir,
        KotlinSourceChanges.ToBeCalculated.INSTANCE,
        new ClasspathChanges.NoChanges(ImmutableList.of()),
        null,
        rebuildReason);
  }

  private static AbsPath path(String relativePath) {
    return TEST_ROOT.resolve(relativePath);
  }
}
