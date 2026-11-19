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

import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.TestStatus;
import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import com.facebook.infer.annotation.Nullsafe;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import org.jspecify.annotations.Nullable;

/**
 * TpxStandardOutputTestListener sends test results to the TPX standard output format. Meant to be
 * shared between different Java test runners that may need to implement specific interfaces.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class TpxStandardOutputTestListener {
  private final TestResultsOutputSender sender;
  private final Map<String, String> resultMetadata;

  private Map<String, TestIdentifierStatus> testIdentifierStatuses = new HashMap<>();
  private Set<String> finishedTests = new HashSet<>();

  // Traces of failures reported without a matching testStarted (suite setup crashes).
  private final List<String> unpairedFailureTraces = new ArrayList<>();

  private class TestIdentifierStatus {
    private long startTime;
    private @Nullable String trace;
    private @Nullable TestStatus status;

    public TestIdentifierStatus(long startTime) {
      this.startTime = startTime;
    }

    public void setFailed(String trace) {
      this.status = TestStatus.FAIL;
      this.trace = trace;
    }

    public void setSkipped(String trace) {
      this.status = TestStatus.SKIP;
      this.trace = trace;
    }

    public void setOmitted(String trace) {
      this.status = TestStatus.OMIT;
      this.trace = trace;
    }
  }

  public TpxStandardOutputTestListener(TestResultsOutputSender sender) {
    this(sender, Collections.emptyMap());
  }

  public TpxStandardOutputTestListener(
      TestResultsOutputSender sender, Map<String, String> resultMetadata) {
    this.sender = sender;
    this.resultMetadata = resultMetadata;
  }

  /**
   * Registers a test identifier with the listener.
   *
   * @param identifier the test identifier to register
   * @param startedTime the time the test started, in milliseconds since Unix epoch
   */
  private void registerTest(String identifier, long startedTime) {
    testIdentifierStatuses.put(identifier, new TestIdentifierStatus(startedTime));
  }

  /**
   * Sends a test start event to the TestResultsOutputSender.
   *
   * @param identifier the test identifier to send the start event for
   * @param startedTime the time the test started, in milliseconds since Unix epoch
   */
  private void sendTestStart(String identifier, long startedTime) {
    sender.sendTestStart(identifier, startedTime);
  }

  /**
   * Reports the start of an individual test case.
   *
   * @param test identifies the test
   */
  public void testStarted(String identifier) {
    if (testIdentifierStatuses.containsKey(identifier)) {
      return;
    }
    long startedTime = System.currentTimeMillis();
    registerTest(identifier, startedTime);
    sendTestStart(identifier, startedTime);
  }

  /**
   * Reports the failure of a individual test case.
   *
   * <p>Will be called between testStarted and testEnded.
   *
   * @param test identifies the test
   * @param trace stack trace of failure
   */
  public void testFailed(String identifier, String trace) {
    TestIdentifierStatus status = testIdentifierStatuses.get(identifier);
    if (status == null) {
      // No matching testStarted means the suite crashed during setup. Capture the trace instead of
      // throwing, which would evict this listener from JUnit's RunNotifier and drop the failure.
      unpairedFailureTraces.add(trace);
      return;
    }

    status.setFailed(trace);
  }

  /** Traces of failures that arrived without a matching testStarted (suite setup crashes). */
  public List<String> getUnpairedFailureTraces() {
    return Collections.unmodifiableList(unpairedFailureTraces);
  }

  /**
   * Called when an atomic test flags that it assumes a condition that is false
   *
   * @param test identifies the test
   * @param trace stack trace of failure
   */
  public void testAssumptionFailure(String identifier, String trace) {
    TestIdentifierStatus status = testIdentifierStatuses.get(identifier);
    if (status == null) {
      throw new IllegalStateException("testAssumptionFailure called without testStarted");
    }

    status.setSkipped(trace);
  }

  /**
   * Called when a test will not be run, generally because a test method is annotated with
   * org.junit.Ignore.
   *
   * @param test identifies the test
   */
  public void testIgnored(String identifier) {
    String reason =
        "Test ignored, generally because the test method is annotated with org.junit.Ignore";
    TestIdentifierStatus status = testIdentifierStatuses.get(identifier);
    if (status == null) {
      // Android instrumentation and JUnit may call testIgnored() without testStarted()
      // for DISABLED_ prefixed tests and @Ignore annotated tests. Synthesize a complete
      // start/finish sequence rather than crashing the entire test run.
      testOmitted(identifier, reason);
      return;
    }

    status.setOmitted(reason);
  }

  /**
   * Reports that a test was omitted (e.g., @Ignore annotation). Unlike testIgnored(), this method
   * handles tests that were filtered out before reaching the listener and creates a complete
   * start/finish sequence with OMIT status so TPX knows not to retry them.
   *
   * @param identifier the test identifier
   * @param reason the reason the test was omitted
   */
  public void testOmitted(String identifier, String reason) {
    long currentTime = System.currentTimeMillis();
    sender.sendTestStart(identifier, currentTime);
    sender.sendTestFinish(
        identifier, TestStatus.OMIT, currentTime, 0, Optional.of(reason), resultMetadata);
  }

  /**
   * Appends captured stdout/stderr to the test's result message so it is surfaced inline alongside
   * the failure trace (e.g. in the terminal when running {@code buck test}). The output is
   * attributed to this specific test, so it does not duplicate across tests the way raw
   * process-level output did. No-op if the test is unknown or the output is empty.
   *
   * <p>Must be called before {@link #testFinished(String)} so the appended output is included in
   * the finish event.
   *
   * @param identifier identifies the test
   * @param output the captured output to append to the test's message
   */
  public void appendTestOutput(String identifier, String output) {
    if (output == null || output.isEmpty()) {
      return;
    }
    TestIdentifierStatus status = testIdentifierStatuses.get(identifier);
    if (status == null) {
      return;
    }
    if (status.trace == null || status.trace.isEmpty()) {
      status.trace = output;
    } else {
      status.trace = status.trace + "\n\n" + output;
    }
  }

  /**
   * Reports the execution end of an individual test case.
   *
   * <p>If {@link #testFailed} was not invoked, this test passed. Also returns any key/value metrics
   * which may have been emitted during the test case's execution.
   *
   * @param test identifies the test
   * @param testMetrics a {@link Map} of the metrics emitted
   */
  public void testFinished(String identifier) {
    long endedTime = System.currentTimeMillis();
    TestIdentifierStatus testIdentifierStatus = testIdentifierStatuses.remove(identifier);
    if (testIdentifierStatus == null) {
      if (finishedTests.contains(identifier)) {
        return;
      }
      throw new IllegalStateException("testEnded called without testStarted for: " + identifier);
    }
    finishedTests.add(identifier);

    long duration = endedTime - testIdentifierStatus.startTime;
    TestStatus resultStatus = TestStatus.PASS;
    if (testIdentifierStatus.status != null) {
      resultStatus = testIdentifierStatus.status;
    }

    sender.sendTestFinish(
        identifier,
        resultStatus,
        endedTime,
        duration,
        Optional.ofNullable(testIdentifierStatus.trace),
        resultMetadata);
  }

  /**
   * Reports test run failed to complete due to a fatal error.
   *
   * @param errorMessage {@link String} describing reason for run failure.
   */
  public void testRunFailed(String errorMessage) {}
}
