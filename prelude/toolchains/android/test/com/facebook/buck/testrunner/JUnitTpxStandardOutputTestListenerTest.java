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

import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.RunFailureStatus;
import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.FileReader;
import java.io.IOException;
import org.junit.Assert;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.junit.runner.Description;
import org.junit.runner.notification.Failure;

/** Tests {@link JUnitTpxStandardOutputListener} */
public class JUnitTpxStandardOutputTestListenerTest {
  File tempFile;

  @Before
  public void setUp() throws IOException {
    tempFile = folder.newFile("test_results.json");
  }

  public JUnitTpxStandardOutputListener createListener(FileOutputStream fileOutputStream) {
    return new JUnitTpxStandardOutputListener(new TestResultsOutputSender(fileOutputStream));
  }

  @Rule public TemporaryFolder folder = new TemporaryFolder();

  @Test
  public void testPassedTest() throws IOException {
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("testOne (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("testOne (TestClass)"));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testFailedTest() throws IOException {
    String testFailureMessage = "failed test trace example";
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      Failure testFailure = new Failure(description, new Throwable(testFailureMessage));
      listener.testFailure(testFailure);
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("testOne (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("testOne (TestClass)"));
      Assert.assertTrue(endLine.contains(testFailureMessage));
      Assert.assertTrue(endLine.contains("\\n")); // stack trace contains new lines

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testStdoutStderrSurfacedInlineOnPassingTest() throws IOException {
    // A test's own System.out/System.err prints should be surfaced inline in the per-test result.
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      // testStarted redirects System.out/System.err into the per-test recorders.
      System.out.println("hello from stdout");
      System.err.println("hello from stderr");
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      reader.readLine(); // start event

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("====TEST STDOUT===="));
      Assert.assertTrue(endLine.contains("hello from stdout"));
      Assert.assertTrue(endLine.contains("====TEST STDERR===="));
      Assert.assertTrue(endLine.contains("hello from stderr"));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testFailedTestIncludesStdoutStderrAfterTrace() throws IOException {
    // On failure, both the stack trace and the test's own stdout/stderr should appear in the
    // per-test message so a developer can see their debug prints next to the failure.
    String testFailureMessage = "failed test trace example";
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      System.err.println("debug print before failure");
      Failure testFailure = new Failure(description, new Throwable(testFailureMessage));
      listener.testFailure(testFailure);
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      reader.readLine(); // start event

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains(testFailureMessage));
      Assert.assertTrue(endLine.contains("====TEST STDERR===="));
      Assert.assertTrue(endLine.contains("debug print before failure"));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testNoInlineOutputWhenTestSilent() throws IOException {
    // A test that prints nothing should not get any output headers added to its result.
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      reader.readLine(); // start event

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertFalse(endLine.contains("====TEST STDOUT===="));
      Assert.assertFalse(endLine.contains("====TEST STDERR===="));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testFailedAssumptionTest() throws IOException {
    String testFailureMessage = "failed test trace example";
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);
      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      Failure testFailure = new Failure(description, new Throwable(testFailureMessage));
      listener.testAssumptionFailure(testFailure);
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("testOne (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("testOne (TestClass)"));
      Assert.assertTrue(endLine.contains(testFailureMessage));
      Assert.assertTrue(endLine.contains("\\n")); // stack trace contains new lines

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testIgnoredTest() throws IOException {
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      listener.testStarted(description);
      listener.testIgnored(description);
      listener.testFinished(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("testOne (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("testOne (TestClass)"));
      Assert.assertTrue(endLine.contains("Test ignored"));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testIgnoredWithoutTestStarted() throws IOException {
    // JUnit calls testIgnored() without testStarted() for @Ignore annotated tests.
    // This must not crash.
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description =
          Description.createTestDescription("TestClass", "DISABLED_testSkipped");
      listener.testIgnored(description);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("DISABLED_testSkipped (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("DISABLED_testSkipped (TestClass)"));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testUnpairedFailureIsCapturedNotEmitted() throws IOException {
    // A setup-crash failure (no matching testStarted) is captured, not thrown, and emits no event.
    String setupTrace = "sandbox init boom";
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description = Description.createTestDescription("TestClass", "testOne");
      Failure failure = new Failure(description, new Throwable(setupTrace));
      listener.testFailure(failure);

      Assert.assertEquals(1, listener.getUnpairedFailureTraces().size());
      Assert.assertTrue(listener.getUnpairedFailureTraces().get(0).contains(setupTrace));
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      Assert.assertNull("Unpaired failure must not emit any event on its own", reader.readLine());
    }
  }

  @Test
  public void testSetupCrashIsReportedAsFatalRunFailure() throws IOException {
    // A setup crash is reported as a FATAL run failure carrying the trace, not a per-case result.
    String trace = "MissingTargetSdkException: sdk 34 not in target_sdk_levels";
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      TestResultsOutputSender sender = new TestResultsOutputSender(fileOutputStream);
      sender.sendRunFailure(RunFailureStatus.FATAL, 123L, "suite crashed during setup", trace);
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String line = reader.readLine();
      Assert.assertTrue(line.contains("run_failure"));
      Assert.assertTrue(line.contains(trace));
      // RunFailureStatus: FATAL == 2. Must be FATAL and not a per-case start/finish event.
      Assert.assertTrue(line.contains("\"status\":2"));
      Assert.assertFalse(line.contains("finish"));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testNoTestsRemainExceptionIsIgnored() throws IOException {
    // When all tests are filtered out (e.g., TPX retries only @Ignore tests),
    // JUnit fires testStarted/testFailure/testFinished for an "initializationError"
    // with className "org.junit.runner.manipulation.Filter". This should be ignored
    // and not reported to TPX.
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      JUnitTpxStandardOutputListener listener = createListener(fileOutputStream);

      Description description =
          Description.createTestDescription(
              "org.junit.runner.manipulation.Filter", "initializationError");
      listener.testStarted(description);
      Failure testFailure =
          new Failure(
              description,
              new Exception(
                  "No tests found matching TestSelectorList-filter from"
                      + " org.junit.runner.Request$1@86733"));
      listener.testFailure(testFailure);
      listener.testFinished(description);
    }

    // The file should be empty - no events should have been sent
    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      Assert.assertNull("Expected no events for NoTestsRemainException", reader.readLine());
    }
  }
}
