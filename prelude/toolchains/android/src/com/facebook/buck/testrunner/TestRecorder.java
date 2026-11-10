/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner;

import com.facebook.infer.annotation.Nullsafe;
import java.io.IOException;
import java.util.logging.Level;
import org.jspecify.annotations.Nullable;

/** Record a test execution, saving the standard outputs and duration. */
@Nullsafe(Nullsafe.Mode.LOCAL)
class TestRecorder {

  private static final String DEBUG_LOGS_HEADER = "====DEBUG LOGS====\n\n";
  private static final String ERROR_LOGS_HEADER = "====ERROR LOGS====\n\n";

  private final StandardOutputRecorder stdOut;
  private final StandardOutputRecorder stdErr;
  private long startTime;
  private long endTime;

  TestRecorder(JUnitOptions options) {
    this.stdOut = StandardOutputRecorder.stdOut(options.getStdOutLogLevel().orElse(Level.INFO));
    this.stdErr = StandardOutputRecorder.stdErr(options.getStdErrLogLevel().orElse(Level.WARNING));
  }

  /**
   * Initialize and start recording standard outputs.
   *
   * @return Current instance.
   */
  public TestRecorder record() {
    try {
      startTime = System.currentTimeMillis();
      stdOut.record();
      stdErr.record();
    } catch (IOException e) {
      throw new IllegalStateException("failed to record standard output", e);
    }
    return this;
  }

  public TestRecorder complete() {
    endTime = System.currentTimeMillis();
    stdOut.complete();
    stdErr.complete();
    return this;
  }

  public long getDuration() {
    return Math.max(endTime - startTime, -1);
  }

  /**
   * Format standard output and log captured during the recording time.
   *
   * @param appendLog to indicate the output should append log content as well.
   * @return recorded standard output.
   */
  public @Nullable String standardOutputAsString(boolean appendLog) {
    return stdOut.toString(appendLog, DEBUG_LOGS_HEADER);
  }

  /**
   * Format standard error output and log captured during the recording time.
   *
   * @param appendLog to indicate the output should append log content as well.
   * @return recorded standard error output.
   */
  public @Nullable String standardErrorAsString(boolean appendLog) {
    return stdErr.toString(appendLog, ERROR_LOGS_HEADER);
  }
}
