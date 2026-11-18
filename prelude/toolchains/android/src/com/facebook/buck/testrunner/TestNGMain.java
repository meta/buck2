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

/**
 * Launcher for TestNG.
 *
 * <p>Expected arguments are:
 *
 * <ul>
 *   <li>(string) output directory
 *   <li>(long) default timeout in milliseconds (0 for no timeout)
 *   <li>(string) newline separated list of test selectors
 *   <li>(string...) fully-qualified names of test classes
 * </ul>
 *
 * <p>IMPORTANT! This class limits itself to types that are available in both the JDK and Android
 * Java API. The objective is to limit the set of files added to the ClassLoader that runs the test,
 * as not to interfere with the results of the test.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class TestNGMain {

  private TestNGMain() {
    // Launcher class.
  }

  public static void main(String[] args) {
    // Ensure that both testng and hamcrest are on the classpath
    CheckDependency.requiresClass("testng", "org.testng.TestNG");
    CheckDependency.requiresClass("hamcrest", "org.hamcrest.Description");

    TestNGRunner runner = new TestNGRunner();
    runner.parseArgs(args);
    runner.runAndExit();
  }
}
