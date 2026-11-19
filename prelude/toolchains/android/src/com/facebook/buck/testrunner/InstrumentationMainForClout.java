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
public class InstrumentationMainForClout {

  private InstrumentationMainForClout() {
    // Launcher class.
  }

  public static void main(String[] args) {
    int exitCode =
        InstrumentationMain.runForExitCode(
            () -> InstrumentationTestRunnerForClout.fromArgs(args).run());
    // Explicitly exit to force the test runner to complete even if tests have sloppily left
    // behind non-daemon threads that would have otherwise forced the process to wait and
    // eventually timeout.
    System.exit(exitCode);
  }
}
