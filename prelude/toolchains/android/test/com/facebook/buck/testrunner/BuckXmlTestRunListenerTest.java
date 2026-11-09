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

import static org.junit.Assert.assertEquals;

import com.android.ddmlib.testrunner.TestIdentifier;
import java.io.File;
import java.util.Collections;
import javax.xml.parsers.DocumentBuilderFactory;
import org.junit.Before;
import org.junit.Rule;
import org.junit.Test;
import org.junit.rules.TemporaryFolder;
import org.w3c.dom.Document;
import org.w3c.dom.Element;

public class BuckXmlTestRunListenerTest {

  @Rule public TemporaryFolder folder = new TemporaryFolder();

  private BuckXmlTestRunListener listener;

  @Before
  public void setUp() {
    listener = new BuckXmlTestRunListener(null, null);
    listener.setReportDir(folder.getRoot());
  }

  @Test
  public void zeroTestRunPreservesTopLevelFailure() throws Exception {
    String failureMessage = "Instrumentation process crashed <before tests> & stopped";

    listener.testRunStarted("zero-test-run", 0);
    listener.testRunFailed(failureMessage);
    listener.testRunEnded(100, Collections.emptyMap());

    Document report = readReport();
    assertEquals(1, report.getElementsByTagName("testsuite").getLength());
    Element testSuite = (Element) report.getElementsByTagName("testsuite").item(0);
    assertEquals("zero-test-run", testSuite.getAttribute("name"));
    assertEquals("0", testSuite.getAttribute("tests"));
    assertEquals("1", testSuite.getAttribute("failures"));
    assertEquals(0, testSuite.getElementsByTagName("testcase").getLength());
    assertEquals(1, testSuite.getElementsByTagName("failure").getLength());
    assertEquals(
        failureMessage, testSuite.getElementsByTagName("failure").item(0).getTextContent());
  }

  @Test
  public void partialTestRunKeepsExistingResultsAndTopLevelFailure() throws Exception {
    String failureMessage = "Instrumentation process crashed after one test";
    TestIdentifier test = new TestIdentifier("ExampleTest", "completedTest");

    listener.testRunStarted("partial-test-run", 2);
    listener.testStarted(test);
    listener.testEnded(test, Collections.emptyMap());
    listener.testRunFailed(failureMessage);
    listener.testRunEnded(200, Collections.emptyMap());

    Document report = readReport();
    assertEquals(1, report.getElementsByTagName("testsuite").getLength());
    Element testSuite = (Element) report.getElementsByTagName("testsuite").item(0);
    assertEquals(1, testSuite.getElementsByTagName("testcase").getLength());
    assertEquals(
        "completedTest",
        ((Element) testSuite.getElementsByTagName("testcase").item(0)).getAttribute("name"));
    assertEquals(1, testSuite.getElementsByTagName("failure").getLength());
    assertEquals(
        failureMessage, testSuite.getElementsByTagName("failure").item(0).getTextContent());
  }

  private Document readReport() throws Exception {
    File resultFile = new File(folder.getRoot(), BuckXmlTestRunListener.TEST_RESULT_FILE);
    return DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(resultFile);
  }
}
