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

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.TestStatus;
import java.io.File;
import java.io.IOException;
import java.nio.file.Files;
import java.util.HashMap;
import java.util.Map;
import java.util.Optional;
import org.junit.Rule;
import org.junit.Test;
import org.junit.contrib.java.lang.system.EnvironmentVariables;
import org.junit.rules.TemporaryFolder;

public class TestResultsOutputSenderTest {
  @Rule public final EnvironmentVariables environmentVariables = new EnvironmentVariables();

  @Rule public TemporaryFolder folder = new TemporaryFolder();

  @Test
  public void testFromEnvNameWithVarSetIsPresent() throws IOException {
    String customEnvVarName = "TEST_CUSTOM_OUTPUT_FILE_NAME";
    File tempFile = folder.newFile("test_results.json");
    environmentVariables.set(customEnvVarName, tempFile.getPath());

    Optional<TestResultsOutputSender> sender =
        TestResultsOutputSender.fromEnvName(customEnvVarName);

    assertTrue(sender.isPresent());
  }

  @Test
  public void testFromEnvWithDefaultVarSetIsPresent() throws IOException {
    File tempFile = folder.newFile("test_results.json");
    environmentVariables.set("TEST_RESULTS_OUTPUT_FILE", tempFile.getPath());

    Optional<TestResultsOutputSender> sender = TestResultsOutputSender.fromDefaultEnvName();

    assertTrue(sender.isPresent());
  }

  @Test
  public void testFromEnvWithVarNotSetIsNotPresent() {
    Optional<TestResultsOutputSender> sender = TestResultsOutputSender.fromDefaultEnvName();

    assertFalse(sender.isPresent());
  }

  private void assertOutputMatchesExpected(String expected, File tempFile) throws IOException {
    try {
      byte[] bytes = Files.readAllBytes(tempFile.toPath());
      String actual = new String(bytes);
      assertEquals(expected, actual);
    } catch (IOException e) {
      throw new RuntimeException("Failed to read file", e);
    }
  }

  @Test
  public void testSendTestStart() throws IOException {
    String customEnvVarName = "TEST_CUSTOM_OUTPUT_FILE_NAME";
    File tempFile = folder.newFile("test_results.json");
    environmentVariables.set(customEnvVarName, tempFile.getPath());

    Optional<TestResultsOutputSender> sender =
        TestResultsOutputSender.fromEnvName(customEnvVarName);
    assertTrue(sender.isPresent());

    long startedTime = 1700000000000L;
    sender.get().sendTestStart("test_test_name", startedTime);

    String expected = "{\"start\":{\"name\":\"test_test_name\",\"started_time\":1700000000000}}\n";

    assertOutputMatchesExpected(expected, tempFile);
  }

  @Test
  public void testSendTestFinish() throws IOException {
    String customEnvVarName = "TEST_CUSTOM_OUTPUT_FILE_NAME";
    File tempFile = folder.newFile("test_results.json");

    environmentVariables.set(customEnvVarName, tempFile.getPath());

    Optional<TestResultsOutputSender> maybeSender =
        TestResultsOutputSender.fromEnvName(customEnvVarName);
    assertTrue(maybeSender.isPresent());

    try (TestResultsOutputSender sender = maybeSender.get()) {
      sender.sendTestFinish(
          "test_test_name", TestStatus.PASS, 20, 5, Optional.ofNullable("test message"));
    }

    String expected =
        "{\"finish\":{\"name\":\"test_test_name\",\"status\":0,\"duration\":5,\"message\":\"test"
            + " message\",\"ended_time\":20}}\n";

    assertOutputMatchesExpected(expected, tempFile);
  }

  @Test
  public void testSendTestFinishNullMessage() throws IOException {
    String customEnvVarName = "TEST_CUSTOM_OUTPUT_FILE_NAME";
    File tempFile = folder.newFile("test_results.json");

    environmentVariables.set(customEnvVarName, tempFile.getPath());

    Optional<TestResultsOutputSender> maybeSender =
        TestResultsOutputSender.fromEnvName(customEnvVarName);
    assertTrue(maybeSender.isPresent());

    try (TestResultsOutputSender sender = maybeSender.get()) {
      sender.sendTestFinish("test_test_name", TestStatus.PASS, 20, 5, Optional.ofNullable(null));
    }

    String expected =
        "{\"finish\":{\"name\":\"test_test_name\",\"status\":0,\"duration\":5,\"ended_time\":20}}\n";

    assertOutputMatchesExpected(expected, tempFile);
  }

  @Test
  public void testSendTestFinishWithMetadata() throws IOException {
    String customEnvVarName = "TEST_CUSTOM_OUTPUT_FILE_NAME";
    File tempFile = folder.newFile("test_results.json");
    environmentVariables.set(customEnvVarName, tempFile.getPath());

    Optional<TestResultsOutputSender> maybeSender =
        TestResultsOutputSender.fromEnvName(customEnvVarName);
    assertTrue(maybeSender.isPresent());

    Map<String, String> metadata = new HashMap<>();
    metadata.put("z_key", "z_value");
    metadata.put("app_info:OS", "Meta/device/build//123:");
    try (TestResultsOutputSender sender = maybeSender.get()) {
      sender.sendTestFinish("test_test_name", TestStatus.PASS, 20, 5, Optional.empty(), metadata);
    }

    String expected =
        "{\"finish\":{\"name\":\"test_test_name\",\"status\":0,\"duration\":5,"
            + "\"ended_time\":20,\"metadata\":{\"app_info:OS\":"
            + "\"Meta/device/build//123:\",\"z_key\":\"z_value\"}}}\n";

    assertOutputMatchesExpected(expected, tempFile);
  }
}
