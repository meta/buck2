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
import com.facebook.buck.android.exopackage.AdbUtils;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.RunFailureStatus;
import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import com.facebook.infer.annotation.Nullsafe;
import java.util.Collections;
import java.util.HashMap;
import java.util.Map;

/**
 * InstrumentationTpxStandardOutputTestListener is a test event listener for Android Instrumentation
 * Test for the TPX Standard Output format.
 *
 * <p>It implements the ITestRunListener interface, which is best documented here:
 * https://developer.android.com/reference/androidx/test/orchestrator/listeners/result/ITestRunListener
 *
 * <p>The sequence of calls copied from the above link, where [ ] indicates optional calls:
 *
 * <p>testRunStarted testStarted [testFailed] [testAssumptionFailure] [testIgnored] testEnded ....
 * [testRunFailed] testRunEnded
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class InstrumentationTpxStandardOutputTestListener implements ITestRunListener {
  private static final String OS_FINGERPRINT_PROPERTY = "ro.build.fingerprint";
  private static final String OS_FINGERPRINT_METADATA_KEY = "app_info:OS";
  private final TpxStandardOutputTestListener listener;
  private final TestResultsOutputSender sender;
  private final AndroidDevice mAndroidDevice;
  private final AdbUtils mAdbUtils;

  public InstrumentationTpxStandardOutputTestListener(
      TestResultsOutputSender sender, AndroidDevice androidDevice, AdbUtils adbUtils) {
    this(sender, androidDevice, adbUtils, false);
  }

  public InstrumentationTpxStandardOutputTestListener(
      TestResultsOutputSender sender,
      AndroidDevice androidDevice,
      AdbUtils adbUtils,
      boolean includeOsFingerprint) {
    this.sender = sender;
    this.listener =
        new TpxStandardOutputTestListener(
            sender,
            includeOsFingerprint ? getResultMetadata(androidDevice) : Collections.emptyMap());
    this.mAndroidDevice = androidDevice;
    this.mAdbUtils = adbUtils;
  }

  static Map<String, String> getResultMetadata(AndroidDevice androidDevice) {
    Map<String, String> metadata = new HashMap<>();
    try {
      String fingerprint = androidDevice.getProperty(OS_FINGERPRINT_PROPERTY);
      if (fingerprint != null && !fingerprint.trim().isEmpty()) {
        metadata.put(OS_FINGERPRINT_METADATA_KEY, fingerprint.trim());
      }
    } catch (Exception e) {
      System.err.printf("Unable to read device OS fingerprint: %s%n", e);
    }
    return metadata;
  }

  /**
   * Gets the full test name for a test identifier.
   *
   * @param test the test identifier to get the name for
   * @return the full test name
   */
  private static String getFullTestName(TestIdentifier test) {
    return String.format("%s (%s)", test.getTestName(), test.getClassName());
  }

  /**
   * Reports the start of an individual test case.
   *
   * @param test identifies the test
   */
  @Override
  public void testStarted(TestIdentifier test) {
    listener.testStarted(getFullTestName(test));
  }

  /**
   * Reports the failure of a individual test case.
   *
   * <p>Will be called between testStarted and testEnded.
   *
   * @param test identifies the test
   * @param trace stack trace of failure
   */
  @Override
  public void testFailed(TestIdentifier test, String trace) {
    if (mAndroidDevice != null && mAdbUtils != null && CrashCapturer.deviceHasCrashLogs(trace)) {
      trace = CrashCapturer.addDeviceLogcatTrace(mAndroidDevice, mAdbUtils, trace);
    }

    listener.testFailed(getFullTestName(test), trace);
  }

  /**
   * Called when an atomic test flags that it assumes a condition that is false
   *
   * @param test identifies the test
   * @param trace stack trace of failure
   */
  @Override
  public void testAssumptionFailure(TestIdentifier test, String trace) {
    listener.testAssumptionFailure(getFullTestName(test), trace);
  }

  /**
   * Called when a test will not be run, generally because a test method is annotated with
   * org.junit.Ignore.
   *
   * @param test identifies the test
   */
  @Override
  public void testIgnored(TestIdentifier test) {
    listener.testIgnored(getFullTestName(test));
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
  @Override
  public void testEnded(TestIdentifier test, Map<String, String> testMetrics) {
    listener.testFinished(getFullTestName(test));
  }

  /**
   * Reports test run failed to complete due to a fatal error.
   *
   * @param errorMessage {@link String} describing reason for run failure.
   */
  @Override
  public void testRunFailed(String errorMessage) {
    // Enhance error message with device logs if available
    String enhancedMessage = errorMessage;
    if (mAndroidDevice != null
        && mAdbUtils != null
        && CrashCapturer.deviceHasCrashLogs(errorMessage)) {
      enhancedMessage = CrashCapturer.addDeviceLogcatTrace(mAndroidDevice, mAdbUtils, errorMessage);
    }

    sender.sendRunFailure(
        RunFailureStatus.FATAL,
        System.currentTimeMillis(),
        enhancedMessage,
        null); // stacktrace is embedded in the message
  }

  /**
   * Reports the start of a test run.
   *
   * @param runName the test run name
   * @param testCount total number of tests in test run
   */
  @Override
  public void testRunStarted(String runName, int testCount) {}

  /**
   * Reports end of test run.
   *
   * @param elapsedTime device reported elapsed time, in milliseconds
   * @param runMetrics key-value pairs reported at the end of a test run
   */
  @Override
  public void testRunEnded(long elapsedTime, Map<String, String> runMetrics) {}

  /**
   * Reports test run stopped before completion due to a user request.
   *
   * <p>Deprecated and not currently used.
   *
   * @param elapsedTime device reported elapsed time, in milliseconds
   */
  @Override
  public void testRunStopped(long elapsedTime) {}
}
