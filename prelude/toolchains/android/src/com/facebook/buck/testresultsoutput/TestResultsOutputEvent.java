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

import com.facebook.infer.annotation.Nullsafe;
import com.fasterxml.jackson.core.JsonFactory;
import com.fasterxml.jackson.core.JsonGenerator;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.util.Collections;
import java.util.Map;
import java.util.Optional;
import java.util.TreeMap;
import org.jspecify.annotations.Nullable;

/**
 * TestResultsOutputEvent implements the following thrift schema.
 * https://www.internalfb.com/code/fbsource/[8bed6a02e7ed]/fbcode/testinfra/if/test_result_output_spec.thrift
 *
 * <p>It keeps and serializes events pertaining to the execution of test cases and replicates
 * instead of using the thrift structs directly to avoid the need for a dependency on fbcode.
 */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class TestResultsOutputEvent {
  private static final JsonFactory jsonFactory = new JsonFactory();

  /** Represents a test event, which can be either a start or finish event. */
  public static class TestEvent {
    public @Nullable StartEvent start;
    public @Nullable FinishEvent finish;
  }

  /** Represents a start event for a test. */
  public static class StartEvent {
    public String name;

    /** The time at which the test started, in milliseconds since Unix epoch. */
    public Long startedTime;

    /**
     * Creates a new start event with the given name and started time.
     *
     * @param name the name of the test
     * @param startedTime the time at which the test started, in milliseconds since Unix epoch
     */
    public StartEvent(String name, long startedTime) {
      this.name = name;
      this.startedTime = startedTime;
    }

    /**
     * Serializes this start event to a JSON byte array.
     *
     * @return the JSON byte array
     * @throws IOException if there is an error serializing the event
     */
    public byte[] toJsonBytes() throws IOException {
      ByteArrayOutputStream outputStream = new ByteArrayOutputStream();
      try (JsonGenerator generator = jsonFactory.createGenerator(outputStream)) {
        generator.writeStartObject();
        generator.writeObjectFieldStart("start");
        generator.writeStringField("name", name);
        generator.writeNumberField("started_time", startedTime);
        generator.writeEndObject();
        generator.writeEndObject();
      }
      return outputStream.toByteArray();
    }
  }

  /** Status of a run failure. */
  public enum RunFailureStatus {
    TIMEOUT(1),
    FATAL(2),
    INFRA_FAILURE(3);

    private final int value;

    RunFailureStatus(int value) {
      this.value = value;
    }

    public int getValue() {
      return value;
    }
  }

  /** Represents a run failure event (not specific to any test case). */
  public static class RunFailureEvent {
    public RunFailureStatus status;
    public long time;
    public String details;
    public @Nullable String stacktrace;

    /**
     * Creates a new run failure event with the given parameters.
     *
     * @param status the status of the run failure (TIMEOUT or FATAL)
     * @param time the time the failure occurred, in milliseconds since Unix epoch
     * @param details human-readable description of the failure
     * @param stacktrace optional stack trace (can be null)
     */
    public RunFailureEvent(
        RunFailureStatus status, long time, String details, @Nullable String stacktrace) {
      this.status = status;
      this.time = time;
      this.details = details;
      this.stacktrace = stacktrace;
    }

    /**
     * Serializes this run failure event to a JSON byte array.
     *
     * @return the JSON byte array
     * @throws IOException if there is an error serializing the event
     */
    public byte[] toJsonBytes() throws IOException {
      ByteArrayOutputStream outputStream = new ByteArrayOutputStream();
      try (JsonGenerator generator = jsonFactory.createGenerator(outputStream)) {
        generator.writeStartObject();
        generator.writeObjectFieldStart("run_failure");
        generator.writeNumberField("status", status.getValue());
        generator.writeNumberField("time", time);
        generator.writeStringField("details", details);
        if (stacktrace != null) {
          generator.writeStringField("stacktrace", stacktrace);
        }
        generator.writeEndObject();
        generator.writeEndObject();
      }
      return outputStream.toByteArray();
    }
  }

  /** Represents the status of a test. */
  public enum TestStatus {
    PASS(0),
    FAIL(1),
    SKIP(2),
    OMIT(4);

    private final int value;

    /**
     * Creates a new test status with the given value.
     *
     * @param value the integer value of the status
     */
    TestStatus(int value) {
      this.value = value;
    }

    /**
     * Returns the integer value of the status.
     *
     * @return the integer value of the status
     */
    public int getValue() {
      return value;
    }
  };

  /** Represents a finish event for a test. */
  public static class FinishEvent {
    public String name;
    public TestStatus status;

    /** The time at which the test ended, in milliseconds since Unix epoch. */
    public long endedTime;

    /** The duration of the test, in milliseconds since the start of the test. */
    public long duration;

    /** A message associated with the test result, or null if there is no message. */
    public String message;

    /** Framework-supplied TestInfra result metadata. */
    public Map<String, String> metadata;

    /**
     * Creates a new finish event with the given parameters.
     *
     * @param name the name of the test
     * @param status the status of the test
     * @param endedTime the time at which the test ended, in milliseconds since Unix epoch
     * @param duration the duration of the test, in milliseconds since the start of the test
     * @param message a message associated with the test result, or null if there is no message
     */
    public FinishEvent(
        String name, TestStatus status, long endedTime, long duration, Optional<String> message) {
      this(name, status, endedTime, duration, message, Collections.emptyMap());
    }

    public FinishEvent(
        String name,
        TestStatus status,
        long endedTime,
        long duration,
        Optional<String> message,
        Map<String, String> metadata) {
      this.name = name;
      this.status = status;
      this.endedTime = endedTime;
      this.duration = duration;
      this.message = message.orElse(null);
      this.metadata = metadata;
    }

    /**
     * Serializes this finish event to a JSON byte array.
     *
     * @return the JSON byte array
     * @throws IOException if there is an error serializing the event
     */
    public byte[] toJsonBytes() throws IOException {
      ByteArrayOutputStream outputStream = new ByteArrayOutputStream();
      try (JsonGenerator generator = jsonFactory.createGenerator(outputStream)) {
        generator.writeStartObject();
        generator.writeObjectFieldStart("finish");
        generator.writeStringField("name", name);
        generator.writeNumberField("status", status.getValue());
        generator.writeNumberField("duration", duration);
        if (message != null) {
          generator.writeStringField("message", message);
        }
        generator.writeNumberField("ended_time", endedTime);
        if (metadata != null && !metadata.isEmpty()) {
          generator.writeObjectFieldStart("metadata");
          for (Map.Entry<String, String> entry : new TreeMap<>(metadata).entrySet()) {
            generator.writeStringField(entry.getKey(), entry.getValue());
          }
          generator.writeEndObject();
        }
        generator.writeEndObject();
        generator.writeEndObject();
      }
      return outputStream.toByteArray();
    }
  }
}
