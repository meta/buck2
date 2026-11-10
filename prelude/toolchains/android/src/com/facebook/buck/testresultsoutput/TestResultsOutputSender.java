/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testresultsoutput;

import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.FinishEvent;
import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.RunFailureEvent;
import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.RunFailureStatus;
import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.StartEvent;
import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.TestStatus;
import com.facebook.infer.annotation.Nullsafe;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;
import java.io.IOException;
import java.util.Collections;
import java.util.Map;
import java.util.Optional;
import org.jspecify.annotations.Nullable;

/**
 * TestResultsOutputSender provides an interface for serializing test_result_output_spec.thrift Test
 * Events.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class TestResultsOutputSender implements AutoCloseable {
  private static final String TEST_RESULTS_OUTPUT_FILE_ENV_NAME = "TEST_RESULTS_OUTPUT_FILE";
  private final FileOutputStream fileOutputStream;

  /**
   * Constructor for TestResultsOutputSender.
   *
   * @param fileOutputStream The FileOutputStream to write to.
   */
  public TestResultsOutputSender(FileOutputStream fileOutputStream) {
    this.fileOutputStream = fileOutputStream;
  }

  @Override
  public void close() throws IOException {
    this.fileOutputStream.close();
  }

  /**
   * Returns a TestResultsOutputSender object if the default environment variable is set.
   *
   * @return An optional TestResultsOutputSender object.
   */
  public static Optional<TestResultsOutputSender> fromDefaultEnvName() {
    return fromEnvName(TEST_RESULTS_OUTPUT_FILE_ENV_NAME);
  }

  @SuppressWarnings("PMD.BlacklistedSystemGetenv")
  private static @Nullable String getEnvValue(String envName) {
    return System.getenv(envName);
  }

  /**
   * Returns a TestResultsOutputSender object if the given environment variable is set.
   *
   * @param envName The name of the environment variable to check.
   * @return An optional TestResultsOutputSender object.
   */
  public static Optional<TestResultsOutputSender> fromEnvName(String envName) {
    return fromFilePath(getEnvValue(envName));
  }

  /**
   * Creates a TestResultsOutputSender object from a file path.
   *
   * @param testResultsOutputFile The file path to create the TestResultsOutputSender from.
   * @return An optional TestResultsOutputSender object.
   */
  public static Optional<TestResultsOutputSender> fromFilePath(
      @Nullable String testResultsOutputFile) {
    if (testResultsOutputFile == null) {
      return Optional.empty();
    }

    try {
      FileOutputStream fileOutputStream = new FileOutputStream(testResultsOutputFile);

      TestResultsOutputSender testResultsOutputSender =
          new TestResultsOutputSender(fileOutputStream);

      return Optional.of(testResultsOutputSender);
    } catch (FileNotFoundException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Sends a test start event to the output file.
   *
   * @param name The name of the test.
   * @param startedTime The time the test started, in milliseconds since Unix epoch.
   */
  public void sendTestStart(String name, long startedTime) {
    StartEvent startEvent = new StartEvent(name, startedTime);

    byte[] serialized;
    try {
      serialized = startEvent.toJsonBytes();
    } catch (IOException e) {
      throw new RuntimeException(e);
    }

    try {
      this.fileOutputStream.write(serialized);
      this.fileOutputStream.write("\n".getBytes());
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Sends a test finish event to the output file.
   *
   * @param name The name of the test.
   * @param status The status of the test.
   * @param endedTime The time the test ended.
   * @param duration The duration of the test.
   * @param message An optional message associated with the test.
   */
  public void sendTestFinish(
      String name, TestStatus status, long endedTime, long duration, Optional<String> message) {
    sendTestFinish(name, status, endedTime, duration, message, Collections.emptyMap());
  }

  public void sendTestFinish(
      String name,
      TestStatus status,
      long endedTime,
      long duration,
      Optional<String> message,
      Map<String, String> metadata) {

    FinishEvent finishEvent = new FinishEvent(name, status, endedTime, duration, message, metadata);

    byte[] serialized;
    try {
      serialized = finishEvent.toJsonBytes();
    } catch (IOException e) {
      throw new RuntimeException(e);
    }

    try {
      this.fileOutputStream.write(serialized);
      this.fileOutputStream.write("\n".getBytes());
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }

  /**
   * Sends a run failure event to the output file.
   *
   * @param status The status of the run failure (TIMEOUT or FATAL).
   * @param time The time the failure occurred, in milliseconds since Unix epoch.
   * @param details Human-readable description of the failure.
   * @param stacktrace Optional stack trace (can be null).
   */
  public void sendRunFailure(
      RunFailureStatus status, long time, String details, @Nullable String stacktrace) {
    RunFailureEvent event = new RunFailureEvent(status, time, details, stacktrace);

    try {
      byte[] serialized = event.toJsonBytes();
      this.fileOutputStream.write(serialized);
      this.fileOutputStream.write("\n".getBytes());
    } catch (IOException e) {
      throw new RuntimeException(e);
    }
  }
}
