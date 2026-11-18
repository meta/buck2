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

@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstrumentationMain {

  @FunctionalInterface
  interface InstrumentationRun {
    void run() throws Throwable;
  }

  private InstrumentationMain() {
    // Launcher class.
  }

  public static void main(String[] args) {
    // Suppress verbose ADB command logging in CI test context to avoid
    // filling details_short with infrastructure noise instead of test output.
    java.util.logging.Logger.getLogger("com.facebook.buck.android.exopackage")
        .setLevel(java.util.logging.Level.WARNING);
    int exitCode = runForExitCode(() -> InstrumentationTestRunner.fromArgs(args).run());
    // Explicitly exit to force the test runner to complete even if tests have sloppily left
    // behind non-daemon threads that would have otherwise forced the process to wait and
    // eventually timeout.
    System.exit(exitCode);
  }

  static int runForExitCode(InstrumentationRun instrumentationRun) {
    try {
      instrumentationRun.run();
      return 0;
    } catch (Throwable e) {
      e.printStackTrace();
      return 1;
    }
  }
}
