/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.test.result.type;

import com.facebook.infer.annotation.Nullsafe;

/** The kind of result */
@Nullsafe(Nullsafe.Mode.LOCAL)
public enum ResultType {

  // First, three different reasons why the tests weren't even attempted:

  /**
   * The test was not run because the user chose to run tests with the --dry-run flag, which caused
   * the test runner to print out the names of tests that *would* have run without actually running
   * them.
   */
  DRY_RUN,
  /** The test was not run because it was excluded by the user */
  EXCLUDED,
  /**
   * The test was not run because it was excluded in source code, such as with JUnit's {@link
   * org.junit.Ignore} annotation or TestNG's {@link org.testng.annotations.Test#enabled()} field.
   */
  DISABLED,

  // Then, three different outcomes when the tests were attempted:

  /**
   * The test was attempted but did not run to completion. It was aborted because a
   * precondition/assumption of the test failed, see {@link org.junit.Assume}.
   */
  ASSUMPTION_VIOLATION,
  /**
   * The test ran, but it failed with either an assertion error or an unexpected uncaught exception.
   * Note that JUnit3 distinguishes between these outcomes (FAILURE and ERROR), while JUnit4 does
   * not.
   */
  FAILURE,
  /** The test ran successfully. */
  SUCCESS
}
