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

import com.android.ddmlib.testrunner.ITestRunListener;
import com.android.ddmlib.testrunner.TestIdentifier;
import com.facebook.infer.annotation.Nullsafe;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;

/**
 * Records the failed tests and the run failures of an instrumentation run, for a launcher that
 * reports them in its exit code instead of through a test executor.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
final class TestFailureRecorder implements ITestRunListener {
  private final List<String> failures = Collections.synchronizedList(new ArrayList<String>());

  /** One entry per failed test or failed run, with its trace or message, in the order seen. */
  List<String> getFailures() {
    synchronized (failures) {
      return new ArrayList<>(failures);
    }
  }

  @Override
  public void testRunStarted(String runName, int testCount) {}

  @Override
  public void testStarted(TestIdentifier test) {}

  @Override
  public void testFailed(TestIdentifier test, String trace) {
    failures.add(test + " failed:\n" + trace);
  }

  @Override
  public void testAssumptionFailure(TestIdentifier test, String trace) {}

  @Override
  public void testIgnored(TestIdentifier test) {}

  @Override
  public void testEnded(TestIdentifier test, Map<String, String> testMetrics) {}

  @Override
  public void testRunFailed(String errorMessage) {
    failures.add("Test run failed: " + errorMessage);
  }

  @Override
  public void testRunStopped(long elapsedTime) {}

  @Override
  public void testRunEnded(long elapsedTime, Map<String, String> runMetrics) {}
}
