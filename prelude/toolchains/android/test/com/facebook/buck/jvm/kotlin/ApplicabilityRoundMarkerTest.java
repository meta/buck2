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

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;
import static org.junit.Assume.assumeFalse;

import com.facebook.buck.testutil.TemporaryPaths;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.PosixFilePermissions;
import org.junit.Rule;
import org.junit.Test;

public class ApplicabilityRoundMarkerTest {
  @Rule public TemporaryPaths temporaryPaths = new TemporaryPaths();

  @Test
  public void aRoundThatStoppedDuringOrAfterACompileDidNotFinish() throws IOException {
    Path stateDir = temporaryPaths.newFolder("incremental_state").getPath();
    assertFalse(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));

    // A round that failed before compiling, or whose compile failed and was rolled back.
    ApplicabilityRoundMarker.enter(stateDir, ApplicabilityRoundMarker.Phase.STARTED);
    assertFalse(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));

    // A worker that stopped during the compile.
    ApplicabilityRoundMarker.enter(stateDir, ApplicabilityRoundMarker.Phase.COMPILING);
    assertTrue(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));

    // An action that failed after its compile was committed.
    ApplicabilityRoundMarker.enter(stateDir, ApplicabilityRoundMarker.Phase.COMPILED);
    assertTrue(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));

    ApplicabilityRoundMarker.clear(stateDir);
    assertFalse(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));
  }

  @Test
  public void aCompileWhoseEndCannotBeRecordedLeavesTheRoundUnfinished() throws IOException {
    Path stateDir = temporaryPaths.newFolder("incremental_state").getPath();
    ApplicabilityRoundMarker.enter(stateDir, ApplicabilityRoundMarker.Phase.COMPILING);
    makeReadOnly(stateDir);
    try {
      // Neither throws: the compile's result stands, and the marker stays at COMPILING.
      ApplicabilityRoundMarker.leaveCompile(stateDir, true);
      ApplicabilityRoundMarker.leaveCompile(stateDir, false);

      assertTrue(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));
    } finally {
      makeWritable(stateDir);
    }
  }

  @Test
  public void aMarkerThatCannotBeClearedIsLeftForTheNextRound() throws IOException {
    Path stateDir = temporaryPaths.newFolder("incremental_state").getPath();
    ApplicabilityRoundMarker.enter(stateDir, ApplicabilityRoundMarker.Phase.COMPILED);
    makeReadOnly(stateDir);
    try {
      ApplicabilityRoundMarker.clear(stateDir);

      assertTrue(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));
    } finally {
      makeWritable(stateDir);
    }
  }

  private static void makeReadOnly(Path dir) throws IOException {
    Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("r-xr-xr-x"));
    if (Files.isWritable(dir)) {
      makeWritable(dir);
      assumeFalse("Running as a user that writes into any directory", true);
    }
  }

  private static void makeWritable(Path dir) throws IOException {
    Files.setPosixFilePermissions(dir, PosixFilePermissions.fromString("rwxr-xr-x"));
  }

  @Test
  public void anUnreadableMarkerCountsAsAnUnfinishedRound() throws IOException {
    Path stateDir = temporaryPaths.newFolder("incremental_state").getPath();
    Files.createDirectory(stateDir.resolve(ApplicabilityRoundMarker.FILE));

    assertTrue(ApplicabilityRoundMarker.lastRoundDidNotFinish(stateDir));
  }
}
