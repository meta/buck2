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

import com.android.ddmlib.testrunner.TestIdentifier;
import com.facebook.buck.android.TestAndroidDevice;
import com.facebook.buck.android.exopackage.AndroidDevice;
import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.FileReader;
import java.io.IOException;
import java.util.HashMap;
import org.junit.Assert;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;

// We basically just want to test that the listener doesn't throw IllegalStateExceptions when it is
// given a correct order of events and that it outputs the same count of lines.
/** Tests {@link InstrumentationTpxStandardOutputTestListener} */
public class InstrumentationTpxStandardOutputTestListenerTest {
  AndroidDevice testDevice;
  File tempFile;

  @Before
  public void setUp() throws IOException {
    testDevice = new TestAndroidDevice();
    tempFile = folder.newFile("test_results.json");
  }

  public InstrumentationTpxStandardOutputTestListener createListener(
      FileOutputStream fileOutputStream) {
    return new InstrumentationTpxStandardOutputTestListener(
        new TestResultsOutputSender(fileOutputStream), testDevice, null);
  }

  @Rule public TemporaryFolder folder = new TemporaryFolder();

  @Test
  public void testPassedTest() throws IOException {
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      InstrumentationTpxStandardOutputTestListener listener = createListener(fileOutputStream);

      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "testOne");
      listener.testStarted(testIdentifier);
      listener.testEnded(testIdentifier, new HashMap<String, String>());
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
      InstrumentationTpxStandardOutputTestListener listener = createListener(fileOutputStream);

      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "testOne");
      listener.testStarted(testIdentifier);
      listener.testFailed(testIdentifier, testFailureMessage);
      listener.testEnded(testIdentifier, new HashMap<String, String>());
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("testOne (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("testOne (TestClass)"));
      Assert.assertTrue(endLine.contains(testFailureMessage));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testFailedAssumptionTest() throws IOException {
    String testFailureMessage = "failed test trace example";
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      InstrumentationTpxStandardOutputTestListener listener = createListener(fileOutputStream);

      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "testOne");
      listener.testStarted(testIdentifier);
      listener.testAssumptionFailure(testIdentifier, testFailureMessage);
      listener.testEnded(testIdentifier, new HashMap<String, String>());
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      String startLine = reader.readLine();
      Assert.assertTrue(startLine.contains("start"));
      Assert.assertTrue(startLine.contains("testOne (TestClass)"));

      String endLine = reader.readLine();
      Assert.assertTrue(endLine.contains("finish"));
      Assert.assertTrue(endLine.contains("testOne (TestClass)"));
      Assert.assertTrue(endLine.contains(testFailureMessage));

      Assert.assertNull(reader.readLine());
    }
  }

  @Test
  public void testIgnoredTest() throws IOException {
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      InstrumentationTpxStandardOutputTestListener listener = createListener(fileOutputStream);

      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "testOne");
      listener.testStarted(testIdentifier);
      listener.testIgnored(testIdentifier);
      listener.testEnded(testIdentifier, new HashMap<String, String>());
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
    // Android instrumentation reports DISABLED_ tests and @Ignore tests by calling
    // testIgnored() without a preceding testStarted(). This must not crash.
    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      InstrumentationTpxStandardOutputTestListener listener = createListener(fileOutputStream);

      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "DISABLED_testSkipped");
      listener.testIgnored(testIdentifier);
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
  public void testReportsDeviceFingerprintAsResultMetadata() throws IOException {
    AndroidDevice device =
        new TestAndroidDevice() {
          @Override
          public String getProperty(String name) {
            return "ro.build.fingerprint".equals(name) ? "Meta/device/build//123:" : null;
          }
        };

    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      InstrumentationTpxStandardOutputTestListener listener =
          new InstrumentationTpxStandardOutputTestListener(
              new TestResultsOutputSender(fileOutputStream), device, null, true);
      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "testOne");
      listener.testStarted(testIdentifier);
      listener.testEnded(testIdentifier, new HashMap<String, String>());
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      reader.readLine();
      String endLine = reader.readLine();
      Assert.assertTrue(
          endLine.contains("\"metadata\":{\"app_info:OS\":\"Meta/device/build//123:\"}"));
    }
  }

  @Test
  public void testDoesNotReadDeviceFingerprintByDefault() throws IOException {
    AndroidDevice device =
        new TestAndroidDevice() {
          @Override
          public String getProperty(String name) {
            throw new AssertionError("OS fingerprint should not be read");
          }
        };

    try (FileOutputStream fileOutputStream = new FileOutputStream(tempFile)) {
      InstrumentationTpxStandardOutputTestListener listener =
          new InstrumentationTpxStandardOutputTestListener(
              new TestResultsOutputSender(fileOutputStream), device, null);
      TestIdentifier testIdentifier = new TestIdentifier("TestClass", "testOne");
      listener.testStarted(testIdentifier);
      listener.testEnded(testIdentifier, new HashMap<String, String>());
    }

    try (BufferedReader reader = new BufferedReader(new FileReader(tempFile))) {
      reader.readLine();
      Assert.assertFalse(reader.readLine().contains("\"metadata\""));
    }
  }

  @Test
  public void testOmitsBlankDeviceFingerprint() {
    AndroidDevice device =
        new TestAndroidDevice() {
          @Override
          public String getProperty(String name) {
            return "   ";
          }
        };

    Assert.assertTrue(
        InstrumentationTpxStandardOutputTestListener.getResultMetadata(device).isEmpty());
  }

  @Test
  public void testOmitsDeviceFingerprintWhenPropertyReadFails() {
    AndroidDevice device =
        new TestAndroidDevice() {
          @Override
          public String getProperty(String name) {
            throw new IllegalStateException("device unavailable");
          }
        };

    Assert.assertTrue(
        InstrumentationTpxStandardOutputTestListener.getResultMetadata(device).isEmpty());
  }
}
