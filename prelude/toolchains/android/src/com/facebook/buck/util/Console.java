/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util;

import com.facebook.buck.core.util.log.Logger;
import com.facebook.infer.annotation.Nullsafe;
import com.google.common.io.ByteStreams;
import java.io.PrintStream;

@Nullsafe(Nullsafe.Mode.LOCAL)
public class Console {

  private static final Logger LOG = Logger.get(Console.class);

  private final Verbosity verbosity;
  private final PrintStream stdOut;
  private final PrintStream stdErr;

  private static final PrintStream NULL_PRINT_STREAM =
      new PrintStream(ByteStreams.nullOutputStream());
  private static final Console NULL_CONSOLE =
      new Console(Verbosity.SILENT, NULL_PRINT_STREAM, NULL_PRINT_STREAM);

  /** Returns a {@link Console} that simply discards written bytes. */
  public static Console createNullConsole() {
    return NULL_CONSOLE;
  }

  public Console(Verbosity verbosity, PrintStream stdOut, PrintStream stdErr) {
    this.verbosity = verbosity;
    this.stdOut = stdOut;
    this.stdErr = stdErr;
  }

  public Verbosity getVerbosity() {
    return verbosity;
  }

  public PrintStream getStdOut() {
    return stdOut;
  }

  public PrintStream getStdErr() {
    return stdErr;
  }

  /** Prints an error message to stderr. */
  public void printErrorText(String message) {
    LOG.warn("Build error: %s", message);
    stdErr.println(message);
  }
}
