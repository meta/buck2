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

import com.facebook.buck.core.util.log.Logger;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.annotations.VisibleForTesting;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;

/**
 * Records how far an incremental round of an action that verifies Kosabi applicability got, so that
 * the next round knows whether the incremental compiler's caches and kotlincd's own round state
 * agree.
 *
 * <p>The incremental compiler commits its caches as soon as a compile succeeds, while kotlincd
 * promotes its pending state (the reduced-classpath fingerprint, the cross-file ledger, the
 * compiler inputs) only once the whole action succeeds. A round that compiled and then failed, or
 * whose worker stopped during the compile, leaves the two out of step, so the next round must be
 * full. A compile that fails cleanly is rolled back by the incremental compiler and leaves them in
 * step.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class ApplicabilityRoundMarker {
  @VisibleForTesting static final String FILE = "applicability_round.txt";

  private static final Logger LOG = Logger.get(ApplicabilityRoundMarker.class);

  /** How far a round got. */
  public enum Phase {
    /** The round started; the incremental compiler has not touched its caches yet. */
    STARTED,
    /** A compile is running. */
    COMPILING,
    /** A compile succeeded and its caches are committed; the action has not finished. */
    COMPILED,
  }

  private ApplicabilityRoundMarker() {}

  /**
   * Whether the last round stopped where the incremental compiler's caches may be ahead of
   * kotlincd's state: during or after a compile, without the action finishing.
   */
  public static boolean lastRoundDidNotFinish(Path incrementalStateDir) {
    Path file = incrementalStateDir.resolve(FILE);
    if (!Files.exists(file)) {
      return false;
    }
    try {
      String contents = Files.readString(file, StandardCharsets.UTF_8);
      // Nullsafe treats the JDK's Files.readString as returning a nullable value.
      if (contents == null) {
        return true;
      }
      return !Phase.STARTED.name().equals(contents.trim());
    } catch (IOException e) {
      return true;
    }
  }

  /** Whether a marker file is there, whatever it records: a compile replaces it. */
  public static boolean isRecorded(Path incrementalStateDir) {
    return Files.isRegularFile(incrementalStateDir.resolve(FILE));
  }

  /** Records that the round reached [phase]. */
  public static void enter(Path incrementalStateDir, Phase phase) throws IOException {
    Files.createDirectories(incrementalStateDir);
    Path temporary = Files.createTempFile(incrementalStateDir, FILE, ".tmp");
    try {
      Files.writeString(temporary, phase.name(), StandardCharsets.UTF_8);
      Files.move(
          temporary,
          incrementalStateDir.resolve(FILE),
          StandardCopyOption.REPLACE_EXISTING,
          StandardCopyOption.ATOMIC_MOVE);
    } finally {
      Files.deleteIfExists(temporary);
    }
  }

  /**
   * Records how a compile ended: COMPILED after a successful one, STARTED after a failed one, which
   * the incremental compiler rolled back. A marker that cannot be written stays at COMPILING, which
   * makes the next round full unless this action succeeds and clears it, so the compile's result
   * stands.
   */
  public static void leaveCompile(Path incrementalStateDir, boolean succeeded) {
    try {
      enter(incrementalStateDir, succeeded ? Phase.COMPILED : Phase.STARTED);
    } catch (IOException e) {
      LOG.warn(e, "Could not record the end of a compile in %s", incrementalStateDir);
    }
  }

  /**
   * Records that the round finished: call only once the action succeeded and promoted its state. A
   * marker that cannot be removed only makes the next round full, so the action still succeeds.
   */
  public static void clear(Path incrementalStateDir) {
    try {
      Files.deleteIfExists(incrementalStateDir.resolve(FILE));
    } catch (IOException e) {
      LOG.warn(e, "Could not remove the applicability round marker in %s", incrementalStateDir);
    }
  }
}
