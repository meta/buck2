/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.remote_worker;

import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.jvm.java.stepsbuilder.javacd.main.JavaCDCommand;
import com.google.common.collect.ImmutableMap;
import java.io.ByteArrayOutputStream;
import java.io.FileNotFoundException;
import java.io.PrintStream;
import org.junit.Test;
import org.kohsuke.args4j.CmdLineException;

public class JavaCDCommandTest {

  @Test
  public void testConstructor_printsUsageToStderrOnMissingRequiredArgs() {
    // The constructor's catch block explicitly prints the error message and usage
    // to stderr before re-throwing CmdLineException. Verify this error handling logic.
    PrintStream originalErr = System.err;
    ByteArrayOutputStream errContent = new ByteArrayOutputStream();
    System.setErr(new PrintStream(errContent));
    try {
      assertThrows(
          CmdLineException.class, () -> new JavaCDCommand(new String[] {}, ImmutableMap.of()));
      String errorOutput = errContent.toString();
      assertTrue(
          "Expected constructor to print error and usage to stderr",
          errorOutput.contains("--action-id"));
    } finally {
      System.setErr(originalErr);
    }
  }

  @Test
  public void testConstructor_throwsFileNotFoundForMissingCommandFile() {
    // After successfully parsing args, the constructor reads the command file.
    // Verify it throws FileNotFoundException with the correct path when the file is missing.
    String missingPath = "/nonexistent/path/to/command.json";
    FileNotFoundException thrown =
        assertThrows(
            FileNotFoundException.class,
            () ->
                new JavaCDCommand(
                    new String[] {"--action-id", "test-action", "--command-file", missingPath},
                    ImmutableMap.of("BUCK_SCRATCH_PATH", "/tmp/scratch")));
    assertTrue(
        "Expected exception message to contain the missing file path",
        thrown.getMessage().contains(missingPath));
  }
}
