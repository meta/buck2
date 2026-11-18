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

import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import com.facebook.buck.testrunner.JavaUtilLoggingHelper.LogHandlers;
import com.facebook.infer.annotation.Nullsafe;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.ConcurrentHashMap;
import java.util.logging.Level;
import org.jspecify.annotations.Nullable;
import org.junit.runner.Description;
import org.junit.runner.notification.Failure;
import org.junit.runner.notification.RunListener;

/**
 * Extends org.junit.runner.notification.RunListener. Meant to be registered with RunNotifier to be
 * notified of events that occurred during a test run.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class JUnitTpxStandardOutputListener extends RunListener {
  private final TpxStandardOutputTestListener listener;
  private final Level stdOutLogLevel = Level.INFO;
  private final Level stdErrLogLevel = Level.WARNING;

  /**
   * Class to hold logging state for each test. This allows tests to run in parallel without sharing
   * logging state.
   */
  private static class TestLogState {
    private ByteArrayOutputStream julLogBytes;
    private ByteArrayOutputStream julErrLogBytes;
    @Nullable private LogHandlers logHandlers;
    @Nullable private StandardOutputRecorder stdoutRecorder;
    @Nullable private StandardOutputRecorder stderrRecorder;

    public TestLogState() {
      this.julLogBytes = new ByteArrayOutputStream();
      this.julErrLogBytes = new ByteArrayOutputStream();
    }

    public ByteArrayOutputStream getJulLogBytes() {
      return julLogBytes;
    }

    public ByteArrayOutputStream getJulErrLogBytes() {
      return julErrLogBytes;
    }

    public void setLogHandlers(LogHandlers logHandlers) {
      this.logHandlers = logHandlers;
    }

    @Nullable
    public LogHandlers getLogHandlers() {
      return logHandlers;
    }

    public void setStdoutRecorder(StandardOutputRecorder stdoutRecorder) {
      this.stdoutRecorder = stdoutRecorder;
    }

    @Nullable
    public StandardOutputRecorder getStdoutRecorder() {
      return stdoutRecorder;
    }

    public void setStderrRecorder(StandardOutputRecorder stderrRecorder) {
      this.stderrRecorder = stderrRecorder;
    }

    @Nullable
    public StandardOutputRecorder getStderrRecorder() {
      return stderrRecorder;
    }
  }

  // Map to store per-test logging state using test description as key.
  // Uses ConcurrentHashMap to support parallel test execution.
  private final Map<String, TestLogState> testLogStates = new ConcurrentHashMap<>();

  public JUnitTpxStandardOutputListener(TestResultsOutputSender sender) {
    this.listener = new TpxStandardOutputTestListener(sender);
  }

  /**
   * Gets the full test name for a org.junit.runner.Description
   *
   * @param test the Description to get the name for
   * @return the full test name
   */
  private static String getFullTestName(Description description) {
    return getFullTestName(description.getMethodName(), description.getClassName());
  }

  /**
   * Gets the full test name from method and class names.
   *
   * @param methodName the test method name
   * @param className the test class name
   * @return the full test name in format "methodName (className)"
   */
  static String getFullTestName(@Nullable String methodName, String className) {
    return String.format("%s (%s)", methodName, className);
  }

  /**
   * Checks if a description represents the NoTestsRemainException error that occurs when all tests
   * in a class are filtered out (e.g., when TPX retries only @Ignore tests). These errors should be
   * ignored and not reported to TPX.
   *
   * @param description the test description to check
   * @return true if this is a NoTestsRemainException error, false otherwise
   */
  private static boolean isNoTestsRemainError(Description description) {
    return "initializationError".equals(description.getMethodName())
        && "org.junit.runner.manipulation.Filter".equals(description.getClassName());
  }

  /**
   * Called when an atomic test is about to be started.
   *
   * @param description the description of the test that is about to be run (generally a class and
   *     method name)
   */
  @Override
  public void testStarted(Description description) {
    if (isNoTestsRemainError(description)) {
      return;
    }

    String testName = getFullTestName(description);

    // Create new test log state for this test
    TestLogState testLogState = new TestLogState();

    // Set up logging handlers
    LogHandlers logHandlers =
        JavaUtilLoggingHelper.setupLogging(
            testLogState.getJulLogBytes(),
            testLogState.getJulErrLogBytes(),
            this.stdOutLogLevel,
            this.stdErrLogLevel);

    testLogState.setLogHandlers(logHandlers);

    // Start capturing this test's stdout/stderr
    try {
      testLogState.setStdoutRecorder(StandardOutputRecorder.stdOut(this.stdOutLogLevel).record());
      testLogState.setStderrRecorder(StandardOutputRecorder.stdErr(this.stdErrLogLevel).record());
    } catch (IOException e) {
      // Log warning but continue - output capture is best-effort
      System.err.println("Warning: Failed to set up stdout/stderr capture: " + e.getMessage());
    }

    // Store the test log state in the map
    testLogStates.put(testName, testLogState);

    listener.testStarted(testName);
  }

  /**
   * Called when an atomic test fails, or when a listener throws an exception.
   *
   * <p>In the case of a failure of an atomic test, this method will be called with the same {@code
   * Description} passed to {@link #testStarted(Description)}, from the same thread that called
   * {@link #testStarted(Description)}.
   *
   * <p>In the case of a listener throwing an exception, this will be called with a {@code
   * Description} of {@link Description#TEST_MECHANISM}, and may be called on an arbitrary thread.
   *
   * @param failure describes the test that failed and the exception that was thrown
   */
  @Override
  public void testFailure(Failure failure) {
    Description description = failure.getDescription();
    if (isNoTestsRemainError(description)) {
      return;
    }
    listener.testFailed(getFullTestName(description), failure.getTrace());
  }

  /**
   * Called when an atomic test flags that it assumes a condition that is false
   *
   * @param failure describes the test that failed and the {@link
   *     org.junit.AssumptionViolatedException} that was thrown
   */
  @Override
  public void testAssumptionFailure(Failure failure) {
    listener.testAssumptionFailure(getFullTestName(failure.getDescription()), failure.getTrace());
  }

  /**
   * Called when a test will not be run, generally because a test method is annotated with {@link
   * org.junit.Ignore}.
   *
   * @param description describes the test that will not be run
   */
  @Override
  public void testIgnored(Description description) {
    listener.testIgnored(getFullTestName(description));
  }

  /**
   * Called when an atomic test has finished, whether the test succeeds or fails.
   *
   * @param description the description of the test that just ran
   */
  @Override
  public void testFinished(Description description) {
    if (isNoTestsRemainError(description)) {
      return;
    }

    String testName = getFullTestName(description);

    // Get the test log state from the map
    TestLogState testLogState = testLogStates.get(testName);

    if (testLogState != null) {
      // Complete stdout/stderr capture
      if (testLogState.getStdoutRecorder() != null) {
        testLogState.getStdoutRecorder().complete();
      }
      if (testLogState.getStderrRecorder() != null) {
        testLogState.getStderrRecorder().complete();
      }

      // Get the captured output (direct prints followed by JUL framework logs under a
      // "====DEBUG LOGS====" / "====ERROR LOGS====" header).
      String stdout =
          testLogState.getStdoutRecorder() != null
              ? testLogState.getStdoutRecorder().toString(true, "====DEBUG LOGS====\n")
              : null;
      String stderr =
          testLogState.getStderrRecorder() != null
              ? testLogState.getStderrRecorder().toString(true, "====ERROR LOGS====\n")
              : null;

      // Surface this test's own stdout/stderr inline in the per-test result so it is visible in
      // the terminal (e.g. next to the failure stack trace).
      String inlineOutput = buildInlineOutput(stdout, stderr);
      if (inlineOutput != null) {
        listener.appendTestOutput(testName, inlineOutput);
      }

      // Clean up logging handlers
      JavaUtilLoggingHelper.cleanupLogging(Objects.requireNonNull(testLogState.getLogHandlers()));

      // Remove the test log state from the map
      testLogStates.remove(testName);
    }

    listener.testFinished(testName);
  }

  /**
   * Builds a string combining this test's captured stdout and stderr (each including the direct
   * prints and the JUL framework logs) for inline display in the per-test result, under "====TEST
   * STDOUT====" / "====TEST STDERR====" headers.
   *
   * @param stdout the captured stdout, or null if there was none
   * @param stderr the captured stderr, or null if there was none
   * @return the combined output, or null if the test produced no output
   */
  @Nullable
  private static String buildInlineOutput(@Nullable String stdout, @Nullable String stderr) {
    StringBuilder sb = new StringBuilder();
    if (stdout != null) {
      sb.append("====TEST STDOUT====\n").append(stdout);
    }
    if (stderr != null) {
      if (sb.length() > 0) {
        sb.append('\n');
      }
      sb.append("====TEST STDERR====\n").append(stderr);
    }
    return sb.length() == 0 ? null : sb.toString();
  }

  /**
   * Reports an omitted test (e.g., @Ignore) to TPX with OMIT status so it won't be retried.
   *
   * @param testName the full test name in format "methodName (className)"
   * @param reason the reason the test was omitted
   */
  public void reportOmittedTest(String testName, String reason) {
    listener.testOmitted(testName, reason);
  }

  /** Traces of failures that arrived without a matching testStarted (suite setup crashes). */
  public List<String> getUnpairedFailureTraces() {
    return listener.getUnpairedFailureTraces();
  }
}
