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

import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertTrue;

import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;

public class CrashAnalyzerTest {

  private CrashAnalyzer crashAnalyzer;
  private ByteArrayOutputStream errContent;
  private PrintStream originalErr;

  @Before
  public void setUp() {
    crashAnalyzer = new CrashAnalyzer();
    errContent = new ByteArrayOutputStream();
    originalErr = System.err;
    System.setErr(new PrintStream(errContent));
  }

  @After
  public void tearDown() {
    System.setErr(originalErr);
  }

  @Test
  public void testDetectsOutOfMemoryError() {
    String logcatOutput =
        "E/AndroidRuntime(12345): FATAL EXCEPTION: main\n"
            + "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed to allocate 100MB\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.SomeClass.method(SomeClass.java:42)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MainActivity.onCreate(MainActivity.java:123)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM error", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue(
        "Should include OOM crash line",
        errorOutput.contains("java.lang.OutOfMemoryError: Failed to allocate 100MB"));
    assertTrue("Should include stack trace", errorOutput.contains("at com.facebook.example"));
  }

  @Test
  public void testDetectsOutOfMemoryErrorWithDifferentMessage() {
    String logcatOutput =
        "E/AndroidRuntime(67890): java.lang.OutOfMemoryError: pthread_create (1040KB stack)"
            + " failed\n"
            + "E/AndroidRuntime(67890): \tat java.lang.Thread.nativeCreate(Native Method)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM error", errorOutput.contains("Out of memory error detected"));
    assertTrue("Should include pthread_create OOM message", errorOutput.contains("pthread_create"));
  }

  @Test
  public void testNoDetectionForNormalLogcat() {
    String logcatOutput =
        "I/ActivityManager(1234): Start proc com.facebook.example\n"
            + "D/MainActivity(5678): onCreate called\n"
            + "I/TestRunner(9012): Test passed successfully\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should always print analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should print no crashes detected", errorOutput.contains("No crashes detected"));
    assertFalse(
        "Should not report OOM for normal logs",
        errorOutput.contains("Out of memory error detected"));
  }

  @Test
  public void testHandlesNullInput() {
    crashAnalyzer.analyzeCrashInformation(null);

    String errorOutput = errContent.toString();
    assertTrue("Should produce no output for null input", errorOutput.isEmpty());
  }

  @Test
  public void testHandlesEmptyInput() {
    crashAnalyzer.analyzeCrashInformation("");

    String errorOutput = errContent.toString();
    assertTrue("Should produce no output for empty input", errorOutput.isEmpty());
  }

  @Test
  public void testHandlesWhitespaceOnlyInput() {
    crashAnalyzer.analyzeCrashInformation("   \n\t\n   ");

    String errorOutput = errContent.toString();
    assertTrue("Should produce no output for whitespace-only input", errorOutput.isEmpty());
  }

  @Test
  public void testDetectsMultipleOOMInSameLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: First OOM\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.OutOfMemoryError: Second OOM\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple OOM errors", errorOutput.contains("Out of memory error detected"));
    assertTrue("Should include first OOM", errorOutput.contains("First OOM"));
  }

  @Test
  public void testOutputContainsEndMarker() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Test error\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.Test.method(Test.java:1)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should include end marker", errorOutput.contains("=== END AIT CRASH ANALYSIS ==="));
  }

  @Test
  public void testOOMWithoutStackTrace() {
    String logcatOutput = "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Allocation failed";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM without stack trace", errorOutput.contains("Allocation failed"));
  }

  @Test
  public void testOOMInDifferentBuffer() {
    String logcatOutput =
        "=== CRASH BUFFER ===\n"
            + "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Buffer overflow\n"
            + "E/AndroidRuntime(12345): \tat com.example.Test(Test.java:100)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM in different buffer", errorOutput.contains("Buffer overflow"));
    assertTrue("Should include AIT detection prefix", errorOutput.contains("AIT_CRASH_DETECTION"));
  }

  @Test
  public void testCaseInsensitiveDetection() {
    String logcatOutput = "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: memory exhausted";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM regardless of case", errorOutput.contains("memory exhausted"));
  }

  @Test
  public void testDetectsOOMWithLongStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed to allocate\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.Class1.method1(Class1.java:10)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.Class2.method2(Class2.java:20)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.Class3.method3(Class3.java:30)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.Class4.method4(Class4.java:40)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.Class5.method5(Class5.java:50)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OOM with long stack trace", errorOutput.contains("Failed to allocate"));
    assertTrue(
        "Should extract at least one stack frame", errorOutput.contains("at com.facebook.example"));
  }

  @Test
  public void testIgnoresNonCrashExceptions() {
    String logcatOutput =
        "W/System.err(12345): java.io.IOException: File not found\n"
            + "W/System.err(12345): \tat"
            + " com.facebook.example.FileReader.read(FileReader.java:42)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should always print analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should print no crashes detected", errorOutput.contains("No crashes detected"));
    assertFalse(
        "Should not detect non-OOM exceptions as crashes",
        errorOutput.contains("Out of memory error detected"));
  }

  @Test
  public void testOOMAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: At end of log";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM at end of logcat", errorOutput.contains("At end of log"));
  }

  @Test
  public void testOOMAtStartOfLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: At start of log\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect OOM at start of logcat", errorOutput.contains("At start of log"));
  }

  @Test
  public void testDetectsStackOverflowError() {
    String logcatOutput =
        "E/AndroidRuntime(12345): FATAL EXCEPTION: main\n"
            + "E/AndroidRuntime(12345): java.lang.StackOverflowError\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.RecursiveClass.recursiveMethod(RecursiveClass.java:42)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.RecursiveClass.recursiveMethod(RecursiveClass.java:42)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue(
        "Should include StackOverflowError", errorOutput.contains("java.lang.StackOverflowError"));
    assertTrue("Should include stack trace", errorOutput.contains("recursiveMethod"));
  }

  @Test
  public void testDetectsStackOverflowErrorWithMessage() {
    String logcatOutput =
        "E/AndroidRuntime(67890): java.lang.StackOverflowError: stack size 8MB\n"
            + "E/AndroidRuntime(67890): \tat java.lang.Thread.run(Thread.java:100)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue("Should include error message", errorOutput.contains("stack size 8MB"));
  }

  @Test
  public void testDetectsStackOverflowErrorWithoutStackTrace() {
    String logcatOutput = "E/AndroidRuntime(12345): java.lang.StackOverflowError";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect StackOverflowError without stack trace",
        errorOutput.contains("Stack overflow error detected"));
    assertTrue("Should include error line", errorOutput.contains("java.lang.StackOverflowError"));
  }

  @Test
  public void testDetectsBothOOMAndStackOverflow() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed allocation\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.StackOverflowError\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OutOfMemoryError", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue("Should include OOM", errorOutput.contains("Failed allocation"));
    assertTrue("Should include SOE", errorOutput.contains("java.lang.StackOverflowError"));
  }

  @Test
  public void testStackOverflowAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "E/AndroidRuntime(12345): java.lang.StackOverflowError";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect StackOverflowError at end of logcat",
        errorOutput.contains("Stack overflow error detected"));
  }

  @Test
  public void testStackOverflowAtStartOfLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.StackOverflowError\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect StackOverflowError at start of logcat",
        errorOutput.contains("Stack overflow error detected"));
  }

  @Test
  public void testStackOverflowWithDeepStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.StackOverflowError\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.A.method(A.java:10)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.B.method(B.java:20)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.C.method(C.java:30)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.A.method(A.java:10)\n"
            + "E/AndroidRuntime(12345): \tat com.facebook.example.B.method(B.java:20)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect StackOverflowError with deep stack trace",
        errorOutput.contains("Stack overflow error detected"));
    assertTrue(
        "Should extract at least one stack frame", errorOutput.contains("at com.facebook.example"));
  }

  @Test
  public void testDetectsNullPointerException() {
    String logcatOutput =
        "E/AndroidRuntime(12345): FATAL EXCEPTION: main\n"
            + "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MyClass.handleObject(MyClass.java:50)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MainActivity.onCreate(MainActivity.java:123)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue(
        "Should include NullPointerException",
        errorOutput.contains("java.lang.NullPointerException"));
    assertTrue("Should include stack trace", errorOutput.contains("handleObject"));
  }

  @Test
  public void testDetectsNullPointerExceptionWithMessage() {
    String logcatOutput =
        "E/AndroidRuntime(67890): java.lang.NullPointerException: Attempt to invoke virtual"
            + " method 'java.lang.String.toString()' on a null object reference\n"
            + "E/AndroidRuntime(67890): \tat com.facebook.example.Utils.process(Utils.java:100)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should include error message", errorOutput.contains("Attempt to invoke virtual method"));
  }

  @Test
  public void testDetectsNullPointerExceptionWithoutStackTrace() {
    String logcatOutput = "E/AndroidRuntime(12345): java.lang.NullPointerException";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException without stack trace",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue("Should include error line", errorOutput.contains("java.lang.NullPointerException"));
  }

  @Test
  public void testDetectsAllThreeErrorTypes() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed allocation\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.StackOverflowError\n"
            + "I/AnotherLog(11111): Another log\n"
            + "E/AndroidRuntime(11122): java.lang.NullPointerException\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OutOfMemoryError", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue("Should include OOM", errorOutput.contains("Failed allocation"));
    assertTrue("Should include SOE", errorOutput.contains("java.lang.StackOverflowError"));
    assertTrue("Should include NPE", errorOutput.contains("java.lang.NullPointerException"));
  }

  @Test
  public void testNullPointerAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "E/AndroidRuntime(12345): java.lang.NullPointerException";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException at end of logcat",
        errorOutput.contains("Null pointer exception detected"));
  }

  @Test
  public void testNullPointerAtStartOfLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException at start of logcat",
        errorOutput.contains("Null pointer exception detected"));
  }

  @Test
  public void testNullPointerWithKotlinStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MyKotlinClass.doSomething(MyKotlinClass.kt:42)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MyKotlinClass$lambda$0(MyKotlinClass.kt:50)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException with Kotlin stack trace",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue("Should include Kotlin file", errorOutput.contains("MyKotlinClass.kt"));
  }

  @Test
  public void testMultipleNullPointerExceptions() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException: First NPE\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.NullPointerException: Second NPE\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple NullPointerExceptions",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue("Should include first NPE", errorOutput.contains("First NPE"));
  }

  @Test
  public void testDetectsArrayIndexOutOfBoundsException() {
    String logcatOutput =
        "E/AndroidRuntime(12345): FATAL EXCEPTION: main\n"
            + "E/AndroidRuntime(12345): java.lang.ArrayIndexOutOfBoundsException: length=5;"
            + " index=10\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.ArrayProcessor.process(ArrayProcessor.java:50)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MainActivity.onCreate(MainActivity.java:123)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ArrayIndexOutOfBoundsException",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue(
        "Should include ArrayIndexOutOfBoundsException",
        errorOutput.contains("java.lang.ArrayIndexOutOfBoundsException"));
    assertTrue("Should include error details", errorOutput.contains("length=5; index=10"));
  }

  @Test
  public void testDetectsIndexOutOfBoundsException() {
    String logcatOutput =
        "E/AndroidRuntime(67890): java.lang.IndexOutOfBoundsException: Index: 10, Size: 3\n"
            + "E/AndroidRuntime(67890): \tat java.util.ArrayList.get(ArrayList.java:437)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IndexOutOfBoundsException",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue("Should include error message", errorOutput.contains("Index: 10, Size: 3"));
  }

  @Test
  public void testDetectsIndexOutOfBoundsExceptionWithoutStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IndexOutOfBoundsException: Index out of range";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IndexOutOfBoundsException without stack trace",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue(
        "Should include error line", errorOutput.contains("java.lang.IndexOutOfBoundsException"));
  }

  @Test
  public void testDetectsAllFourErrorTypes() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed allocation\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.StackOverflowError\n"
            + "I/AnotherLog(11111): Another log\n"
            + "E/AndroidRuntime(11122): java.lang.NullPointerException\n"
            + "I/MoreLog(22222): More log\n"
            + "E/AndroidRuntime(33344): java.lang.ArrayIndexOutOfBoundsException: index=5\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OutOfMemoryError", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect ArrayIndexOutOfBoundsException",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue("Should include OOM", errorOutput.contains("Failed allocation"));
    assertTrue("Should include SOE", errorOutput.contains("java.lang.StackOverflowError"));
    assertTrue("Should include NPE", errorOutput.contains("java.lang.NullPointerException"));
    assertTrue(
        "Should include AIOOBE", errorOutput.contains("java.lang.ArrayIndexOutOfBoundsException"));
  }

  @Test
  public void testIndexOutOfBoundsAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "E/AndroidRuntime(12345): java.lang.IndexOutOfBoundsException: Index: 5, Size: 3";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IndexOutOfBoundsException at end of logcat",
        errorOutput.contains("Index out of bounds exception detected"));
  }

  @Test
  public void testIndexOutOfBoundsAtStartOfLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ArrayIndexOutOfBoundsException: index=10\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ArrayIndexOutOfBoundsException at start of logcat",
        errorOutput.contains("Index out of bounds exception detected"));
  }

  @Test
  public void testIndexOutOfBoundsWithKotlinStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IndexOutOfBoundsException\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MyKotlinList.getItem(MyKotlinList.kt:42)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MyKotlinList$process$1.invoke(MyKotlinList.kt:50)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IndexOutOfBoundsException with Kotlin stack trace",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue("Should include Kotlin file", errorOutput.contains("MyKotlinList.kt"));
  }

  @Test
  public void testMultipleIndexOutOfBoundsExceptions() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ArrayIndexOutOfBoundsException: First error\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.IndexOutOfBoundsException: Second error\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple IndexOutOfBoundsExceptions",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue("Should include first error", errorOutput.contains("First error"));
  }

  @Test
  public void testArrayIndexOutOfBoundsWithNegativeIndex() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ArrayIndexOutOfBoundsException: length=5; index=-1\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.ArrayHandler.get(ArrayHandler.java:10)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect negative index error",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue("Should include negative index", errorOutput.contains("index=-1"));
  }

  @Test
  public void testDetectsIllegalStateException() {
    String logcatOutput =
        "E/AndroidRuntime(12345): FATAL EXCEPTION: main\n"
            + "E/AndroidRuntime(12345): java.lang.IllegalStateException: Test illegal state crash\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.StateManager.doAction(StateManager.java:50)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MainActivity.onCreate(MainActivity.java:123)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IllegalStateException",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue(
        "Should include IllegalStateException",
        errorOutput.contains("java.lang.IllegalStateException"));
    assertTrue("Should include error message", errorOutput.contains("Test illegal state crash"));
  }

  @Test
  public void testDetectsIllegalStateExceptionWithDetailedMessage() {
    String logcatOutput =
        "E/AndroidRuntime(67890): java.lang.IllegalStateException: Fragment not attached to"
            + " Activity\n"
            + "E/AndroidRuntime(67890): \tat"
            + " androidx.fragment.app.Fragment.getContext(Fragment.java:100)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IllegalStateException",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue(
        "Should include error message", errorOutput.contains("Fragment not attached to Activity"));
  }

  @Test
  public void testDetectsIllegalStateExceptionWithoutStackTrace() {
    String logcatOutput = "E/AndroidRuntime(12345): java.lang.IllegalStateException: Invalid state";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IllegalStateException without stack trace",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue(
        "Should include error line", errorOutput.contains("java.lang.IllegalStateException"));
  }

  @Test
  public void testDetectsAllFiveErrorTypes() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed allocation\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.StackOverflowError\n"
            + "I/AnotherLog(11111): Another log\n"
            + "E/AndroidRuntime(11122): java.lang.NullPointerException\n"
            + "I/MoreLog(22222): More log\n"
            + "E/AndroidRuntime(33344): java.lang.ArrayIndexOutOfBoundsException: index=5\n"
            + "I/EvenMoreLog(44444): Even more log\n"
            + "E/AndroidRuntime(55566): java.lang.IllegalStateException: Invalid state\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OutOfMemoryError", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect ArrayIndexOutOfBoundsException",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue(
        "Should detect IllegalStateException",
        errorOutput.contains("Illegal state exception detected"));
  }

  @Test
  public void testIllegalStateAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "E/AndroidRuntime(12345): java.lang.IllegalStateException: At end";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IllegalStateException at end of logcat",
        errorOutput.contains("Illegal state exception detected"));
  }

  @Test
  public void testIllegalStateAtStartOfLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IllegalStateException: At start\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IllegalStateException at start of logcat",
        errorOutput.contains("Illegal state exception detected"));
  }

  @Test
  public void testIllegalStateWithKotlinStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IllegalStateException: Coroutine state error\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.MyCoroutine.execute(MyCoroutine.kt:42)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " kotlinx.coroutines.CoroutineScope.launch(CoroutineScope.kt:50)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect IllegalStateException with Kotlin stack trace",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue("Should include Kotlin file", errorOutput.contains("MyCoroutine.kt"));
  }

  @Test
  public void testMultipleIllegalStateExceptions() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IllegalStateException: First error\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.IllegalStateException: Second error\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple IllegalStateExceptions",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue("Should include first error", errorOutput.contains("First error"));
  }

  @Test
  public void testIllegalStateWithFragmentError() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IllegalStateException: Can not perform this action"
            + " after onSaveInstanceState\n"
            + "E/AndroidRuntime(12345): \tat"
            + " androidx.fragment.app.FragmentManager.checkStateLoss(FragmentManager.java:1500)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect fragment IllegalStateException",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue(
        "Should include fragment error message", errorOutput.contains("after onSaveInstanceState"));
  }

  @Test
  public void testIllegalStateWithViewError() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.IllegalStateException: The specified child already"
            + " has a parent\n"
            + "E/AndroidRuntime(12345): \tat"
            + " android.view.ViewGroup.addViewInner(ViewGroup.java:4950)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect view IllegalStateException",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue("Should include view error message", errorOutput.contains("already has a parent"));
  }

  @Test
  public void testDetectsSIGABRTSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345 (com.facebook.app)\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_x86_64/generic_x86_64:11'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00000000000a1234  /system/lib64/libc.so (abort+123)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect SIGABRT signal", errorOutput.contains("SIGABRT signal detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should include signal line", errorOutput.contains("Fatal signal 6"));
    assertTrue("Should include backtrace", errorOutput.contains("backtrace:"));
  }

  @Test
  public void testDetectsSIGABRTWithUppercase() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT with uppercase", errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include signal info", errorOutput.contains("SIGABRT"));
  }

  @Test
  public void testDetectsSIGABRTWithLowercase() {
    String logcatOutput =
        "F/libc(12345): fatal signal 6 (sigabrt), code -6 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT case-insensitively",
        errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testDetectsSignal6() {
    String logcatOutput =
        "F/libc(12345): signal 6 received\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect signal 6", errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include signal info", errorOutput.contains("signal 6"));
  }

  @Test
  public void testDetectsAbortKeyword() {
    String logcatOutput =
        "F/libc(12345): Abort message: 'some error message'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect Abort keyword", errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include abort message", errorOutput.contains("Abort message"));
  }

  @Test
  public void testSIGABRTWithMultipleBacktraceLines() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so (abort+123)\n"
            + "I/DEBUG(1234):     #01 pc 00005678  /system/lib64/libc.so (raise+45)\n"
            + "I/DEBUG(1234):     #02 pc 00009abc "
            + " /data/app/com.facebook.app/lib/x86_64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT with multiple backtrace lines",
        errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include first backtrace line", errorOutput.contains("#00 pc"));
  }

  @Test
  public void testSIGABRTWithNativeLibrary() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT with native library",
        errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include native library", errorOutput.contains("libnative.so"));
  }

  @Test
  public void testSIGABRTAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT at end of logcat", errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testSIGABRTAtStartOfLogcat() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT at start of logcat",
        errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testSIGABRTWithDebugInfo() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345 (Thread-5)\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_x86_64/generic_x86_64:11'\n"
            + "I/DEBUG(1234): Revision: '0'\n"
            + "I/DEBUG(1234): ABI: 'x86_64'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT with debug info", errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include thread name", errorOutput.contains("Thread-5"));
  }

  @Test
  public void testMultipleSIGABRTOccurrences() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 6 (SIGABRT), code -6 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple SIGABRT occurrences",
        errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testSIGABRTWithExceptionAndSignal() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 6 (SIGABRT), code -6 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue("Should detect SIGABRT signal", errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testSIGABRTWithPcAndSymbol() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00012345  /system/lib64/libc.so (abort+256)\n"
            + "I/DEBUG(1234):     #01 pc 00067890  /system/lib64/libc.so (__assert_fail+128)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGABRT with symbols", errorOutput.contains("SIGABRT signal detected"));
    assertTrue("Should include pc info", errorOutput.contains("pc 00012345"));
  }

  @Test
  public void testDetectsSIGSEGVSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_x86_64/generic_x86_64:11'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should include signal line", errorOutput.contains("Fatal signal 11"));
    assertTrue("Should include backtrace", errorOutput.contains("backtrace:"));
  }

  @Test
  public void testDetectsSIGSEGVWithUppercase() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with uppercase",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include signal info", errorOutput.contains("SIGSEGV"));
  }

  @Test
  public void testDetectsSIGSEGVWithLowercase() {
    String logcatOutput =
        "F/libc(12345): fatal signal 11 (sigsegv), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV case-insensitively",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
  }

  @Test
  public void testDetectsSignal11() {
    String logcatOutput =
        "F/libc(12345): signal 11 received\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect signal 11", errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include signal info", errorOutput.contains("signal 11"));
  }

  @Test
  public void testDetectsSegmentationFaultKeyword() {
    String logcatOutput =
        "F/libc(12345): Segmentation fault\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect segmentation fault keyword",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue(
        "Should include segmentation fault text", errorOutput.contains("Segmentation fault"));
  }

  @Test
  public void testDoesNotFalsePositiveOnMqdSigsegvWarning() {
    // Regression: the benign MessageExecutionMonitor warning (added by D109574690) mentions the
    // word "SIGSEGV" and fires hundreds of times per run, but is NOT a native crash. It must not be
    // detected as one (previously the bare "SIGSEGV" alternative matched it and marked runs Fatal).
    String logcatOutput =
        "I/ActivityManager(1234): Start proc com.facebook.example\n"
            + "W/MessageExecutionMonitor(4618): Skipping getStackTrace: target thread state"
            + " RUNNABLE, MQD callback delayed by 0 ms; risk of ART nterp stack-walk SIGSEGV /"
            + " suspension timeout\n"
            + "I/TestRunner(9012): Test passed successfully\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertFalse(
        "Benign MessageExecutionMonitor SIGSEGV warning must not be detected as a native crash",
        errorOutput.contains("SIGSEGV signal detected"));
    assertTrue(
        "Should report no crashes for the benign warning",
        errorOutput.contains("No crashes detected"));
  }

  @Test
  public void testDoesNotFalsePositiveOnAbortHookModuleLoad() {
    // Regression: WhatsApp logs "Loading module: abort_hook" at every app start. It is not a
    // SIGABRT and must not be detected as one (previously the bare "Abort" alternative matched it).
    String logcatOutput =
        "10-14 01:35:04.592  5977  5977 I WhatsApp: LL_I D [2:main]SafeModuleLoader.java:122"
            + " Loading module: abort_hook\n"
            + "I/TestRunner(9012): Test passed successfully\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertFalse(
        "abort_hook module load must not be detected as a SIGABRT",
        errorOutput.contains("SIGABRT signal detected"));
    assertTrue(
        "Should report no crashes for the module load",
        errorOutput.contains("No crashes detected"));
  }

  @Test
  public void testDoesNotFalsePositiveOnSignal60() {
    String logcatOutput = "I/Example(1234): received signal 60 from peer\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    assertFalse(
        "signal 60 must not be detected as a SIGABRT",
        errContent.toString().contains("SIGABRT signal detected"));
  }

  @Test
  public void testDoesNotFalsePositiveOnQplIllegalStateWarning() {
    // Regression: QPLProvider logs a caught IllegalStateException ("No QPL instance provided") at
    // error level under its own tag on many runs. It is not an uncaught crash and must not be
    // detected as one (previously the bare java.lang.IllegalStateException match flagged it).
    String logcatOutput =
        "07-15 15:16:17.261 20137 20205 I ActivityManager: Start proc com.oculus.assistant\n"
            + "07-15 15:16:17.261 20137 20205 E QPLProvider: java.lang.IllegalStateException: No"
            + " QPL instance provided\n"
            + "07-15 15:16:17.261 20137 20205 E QPLProvider: \tat"
            + " com.facebook.quicklog.QuickPerformanceLoggerProvider.getQPLInstance(Unknown"
            + " Source:8)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertFalse(
        "Caught QPLProvider IllegalStateException must not be detected as a crash",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue(
        "Should report no crashes for the benign QPL warning",
        errorOutput.contains("No crashes detected"));
  }

  @Test
  public void testDoesNotFalsePositiveOnCoverageClassNotFound() {
    // Regression: CoverageInit logs a caught ClassNotFoundException at debug level under its own
    // tag
    // on instrumented runs. It is not an uncaught crash and must not be detected as one (previously
    // the bare java.lang.ClassNotFoundException match flagged it).
    String logcatOutput =
        "07-15 15:16:15.868 19867 19867 D CoverageInit: java.lang.ClassNotFoundException:"
            + " com.facebook.coverage.e2e.CoverageSetup\n"
            + "07-15 15:16:15.868 19867 19867 D CoverageInit: \tat"
            + " java.lang.Class.classForName(Native Method)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertFalse(
        "Caught CoverageInit ClassNotFoundException must not be detected as a crash",
        errorOutput.contains("Class not found exception detected"));
    assertTrue(
        "Should report no crashes for the benign Coverage debug log",
        errorOutput.contains("No crashes detected"));
  }

  @Test
  public void testSIGSEGVWithNullPointerDereference() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0 in tid"
            + " 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so"
            + " (Java_com_facebook_native_crash+42)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with null pointer dereference",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include fault address", errorOutput.contains("fault addr 0x0"));
    assertTrue("Should include SEGV_MAPERR", errorOutput.contains("SEGV_MAPERR"));
  }

  @Test
  public void testSIGSEGVWithInvalidAddress() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0xdeadbeef\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with invalid address",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include fault address", errorOutput.contains("fault addr 0xdeadbeef"));
  }

  @Test
  public void testSIGSEGVWithMultipleBacktraceLines() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (nativeMethod+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (callNative+45)\n"
            + "I/DEBUG(1234):     #02 pc 00009abc  /system/lib64/libart.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with multiple backtrace lines",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include first backtrace line", errorOutput.contains("#00 pc"));
  }

  @Test
  public void testSIGSEGVWithNativeLibrary() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libfacebook.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with native library",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include native library", errorOutput.contains("libfacebook.so"));
  }

  @Test
  public void testSIGSEGVAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV at end of logcat",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
  }

  @Test
  public void testSIGSEGVAtStartOfLogcat() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV at start of logcat",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
  }

  @Test
  public void testSIGSEGVWithDebugInfo() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0 in tid"
            + " 12345 (NativeThread)\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_arm64/generic_arm64:12'\n"
            + "I/DEBUG(1234): Revision: '0'\n"
            + "I/DEBUG(1234): ABI: 'arm64'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with debug info",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include thread name", errorOutput.contains("NativeThread"));
  }

  @Test
  public void testMultipleSIGSEGVOccurrences() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 11 (SIGSEGV), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple SIGSEGV occurrences",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
  }

  @Test
  public void testSIGSEGVWithExceptionAndSignals() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 11 (SIGSEGV), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
  }

  @Test
  public void testSIGSEGVAndSIGABRTTogether() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 6 (SIGABRT), code -6 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should detect SIGABRT signal", errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testSIGSEGVWithPcAndSymbol() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00012345  /system/lib64/libc.so (strcmp+256)\n"
            + "I/DEBUG(1234):     #01 pc 00067890 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (processString+128)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with symbols",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include pc info", errorOutput.contains("pc 00012345"));
  }

  @Test
  public void testSIGSEGVWithSEGVACCERR() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 2 (SEGV_ACCERR), fault addr 0x12345678 in"
            + " tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV with SEGV_ACCERR",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include SEGV_ACCERR", errorOutput.contains("SEGV_ACCERR"));
    assertTrue("Should include fault address", errorOutput.contains("fault addr 0x12345678"));
  }

  @Test
  public void testSIGSEGVWithJNICall() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 11 (SIGSEGV), code 1 (SEGV_MAPERR), fault addr 0x0\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so"
            + " (Java_com_facebook_jni_NativeClass_nativeMethod+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678  /system/lib64/libart.so"
            + " (art_quick_generic_jni_trampoline+152)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGSEGV in JNI call",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should include JNI method name", errorOutput.contains("Java_com_facebook_jni"));
  }

  @Test
  public void testSegmentationFaultCaseInsensitive() {
    String logcatOutput =
        "F/libc(12345): SEGMENTATION FAULT\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect segmentation fault case-insensitively",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
  }

  @Test
  public void testDetectsSIGILLSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 (ILL_ILLOPC), fault addr 0x12345678\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_x86_64/generic_x86_64:11'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should include signal line", errorOutput.contains("Fatal signal 4"));
    assertTrue("Should include backtrace", errorOutput.contains("backtrace:"));
  }

  @Test
  public void testDetectsSIGILLWithUppercase() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with uppercase",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include signal info", errorOutput.contains("SIGILL"));
  }

  @Test
  public void testDetectsSIGILLWithLowercase() {
    String logcatOutput =
        "F/libc(12345): fatal signal 4 (sigill), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL case-insensitively",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testDetectsSignal4() {
    String logcatOutput =
        "F/libc(12345): signal 4 received\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect signal 4",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include signal info", errorOutput.contains("signal 4"));
  }

  @Test
  public void testDetectsIllegalInstructionKeyword() {
    String logcatOutput =
        "F/libc(12345): Illegal instruction\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect illegal instruction keyword",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue(
        "Should include illegal instruction text", errorOutput.contains("Illegal instruction"));
  }

  @Test
  public void testSIGILLWithILL_ILLOPC() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 (ILL_ILLOPC), fault addr 0x12345678 in"
            + " tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with ILL_ILLOPC",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include fault address", errorOutput.contains("fault addr 0x12345678"));
    assertTrue("Should include ILL_ILLOPC", errorOutput.contains("ILL_ILLOPC"));
  }

  @Test
  public void testSIGILLWithILL_ILLOPN() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 2 (ILL_ILLOPN), fault addr 0xabcdef00\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with ILL_ILLOPN",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include ILL_ILLOPN", errorOutput.contains("ILL_ILLOPN"));
  }

  @Test
  public void testSIGILLWithMultipleBacktraceLines() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (executeCode+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (runFunction+45)\n"
            + "I/DEBUG(1234):     #02 pc 00009abc  /system/lib64/libart.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with multiple backtrace lines",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include first backtrace line", errorOutput.contains("#00 pc"));
  }

  @Test
  public void testSIGILLWithNativeLibrary() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libfacebook.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with native library",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include native library", errorOutput.contains("libfacebook.so"));
  }

  @Test
  public void testSIGILLAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL at end of logcat",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGILLAtStartOfLogcat() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL at start of logcat",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGILLWithDebugInfo() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 (ILL_ILLOPC), fault addr 0x1234 in tid"
            + " 12345 (ExecutorThread)\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_arm64/generic_arm64:12'\n"
            + "I/DEBUG(1234): Revision: '0'\n"
            + "I/DEBUG(1234): ABI: 'arm64'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with debug info",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include thread name", errorOutput.contains("ExecutorThread"));
  }

  @Test
  public void testMultipleSIGILLOccurrences() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 4 (SIGILL), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple SIGILL occurrences",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGILLWithExceptionAndSignals() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 4 (SIGILL), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGILLAndOtherSignalsTogether() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 11 (SIGSEGV), code 1 in tid 67890\n"
            + "I/MoreLog(11111): More log\n"
            + "F/libc(33344): Fatal signal 6 (SIGABRT), code -6 in tid 33344\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue("Should detect SIGABRT signal", errorOutput.contains("SIGABRT signal detected"));
  }

  @Test
  public void testSIGILLWithPcAndSymbol() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00012345  /system/lib64/libc.so (someFunction+256)\n"
            + "I/DEBUG(1234):     #01 pc 00067890 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (executeInstruction+128)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with symbols",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include pc info", errorOutput.contains("pc 00012345"));
  }

  @Test
  public void testSIGILLWithJNICall() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 1 (ILL_ILLOPC), fault addr 0x1234\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so"
            + " (Java_com_facebook_jni_NativeClass_executeNative+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678  /system/lib64/libart.so"
            + " (art_quick_generic_jni_trampoline+152)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL in JNI call",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include JNI method name", errorOutput.contains("Java_com_facebook_jni"));
  }

  @Test
  public void testIllegalInstructionCaseInsensitive() {
    String logcatOutput =
        "F/libc(12345): ILLEGAL INSTRUCTION\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect illegal instruction case-insensitively",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGILLWithUserSentSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 0 (SI_USER) in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL sent by user",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include SI_USER", errorOutput.contains("SI_USER"));
  }

  @Test
  public void testSIGILLWithILL_PRVOPC() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 4 (SIGILL), code 3 (ILL_PRVOPC), fault addr 0x1000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGILL with ILL_PRVOPC",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue("Should include ILL_PRVOPC", errorOutput.contains("ILL_PRVOPC"));
  }

  @Test
  public void testDetectsSIGFPESignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 (FPE_INTDIV), fault addr 0x12345678\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_x86_64/generic_x86_64:11'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE signal",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should include signal line", errorOutput.contains("Fatal signal 8"));
    assertTrue("Should include backtrace", errorOutput.contains("backtrace:"));
  }

  @Test
  public void testDetectsSIGFPEWithUppercase() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with uppercase",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include signal info", errorOutput.contains("SIGFPE"));
  }

  @Test
  public void testDetectsSIGFPEWithLowercase() {
    String logcatOutput =
        "F/libc(12345): fatal signal 8 (sigfpe), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE case-insensitively",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
  }

  @Test
  public void testDetectsSignal8() {
    String logcatOutput =
        "F/libc(12345): signal 8 received\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect signal 8",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include signal info", errorOutput.contains("signal 8"));
  }

  @Test
  public void testDetectsFloatingPointExceptionKeyword() {
    String logcatOutput =
        "F/libc(12345): Floating point exception\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect floating point exception keyword",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue(
        "Should include floating point exception text",
        errorOutput.contains("Floating point exception"));
  }

  @Test
  public void testSIGFPEWithFPE_INTDIV() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 (FPE_INTDIV), fault addr 0x12345678 in"
            + " tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_INTDIV",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("integer divide by zero"));
    assertTrue("Should include FPE_INTDIV", errorOutput.contains("FPE_INTDIV"));
  }

  @Test
  public void testSIGFPEWithFPE_INTOVF() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 2 (FPE_INTOVF), fault addr 0xabcdef00\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_INTOVF",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("integer overflow"));
  }

  @Test
  public void testSIGFPEWithFPE_FLTDIV() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 3 (FPE_FLTDIV), fault addr 0x1000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_FLTDIV",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("floating point divide by zero"));
  }

  @Test
  public void testSIGFPEWithFPE_FLTOVF() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 4 (FPE_FLTOVF), fault addr 0x2000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_FLTOVF",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("floating point overflow"));
  }

  @Test
  public void testSIGFPEWithFPE_FLTUND() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 5 (FPE_FLTUND), fault addr 0x3000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_FLTUND",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("floating point underflow"));
  }

  @Test
  public void testSIGFPEWithMultipleBacktraceLines() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (divideNumbers+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (calculate+45)\n"
            + "I/DEBUG(1234):     #02 pc 00009abc  /system/lib64/libart.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with multiple backtrace lines",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include first backtrace line", errorOutput.contains("#00 pc"));
  }

  @Test
  public void testSIGFPEAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 in tid 12345";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE at end of logcat",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
  }

  @Test
  public void testSIGFPEAtStartOfLogcat() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 in tid 12345\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE at start of logcat",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
  }

  @Test
  public void testMultipleSIGFPEOccurrences() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 8 (SIGFPE), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple SIGFPE occurrences",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
  }

  @Test
  public void testSIGFPEWithExceptionAndSignals() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 8 (SIGFPE), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect SIGFPE signal",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
  }

  @Test
  public void testSIGFPEAndOtherSignalsTogether() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 11 (SIGSEGV), code 1 in tid 67890\n"
            + "I/MoreLog(11111): More log\n"
            + "F/libc(33344): Fatal signal 4 (SIGILL), code 1 in tid 33344\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE signal",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGFPEWithJNICall() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 1 (FPE_INTDIV), fault addr 0x1234\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so"
            + " (Java_com_facebook_jni_NativeClass_divide+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678  /system/lib64/libart.so"
            + " (art_quick_generic_jni_trampoline+152)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE in JNI call",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include JNI method name", errorOutput.contains("Java_com_facebook_jni"));
  }

  @Test
  public void testFloatingPointExceptionCaseInsensitive() {
    String logcatOutput =
        "F/libc(12345): FLOATING POINT EXCEPTION\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect floating point exception case-insensitively",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
  }

  @Test
  public void testSIGFPEWithUserSentSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 0 (SI_USER) in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE sent by user",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include SI_USER", errorOutput.contains("SI_USER"));
  }

  @Test
  public void testSIGFPEWithFPE_FLTRES() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 6 (FPE_FLTRES), fault addr 0x4000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_FLTRES",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("inexact result"));
  }

  @Test
  public void testSIGFPEWithFPE_FLTINV() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 8 (SIGFPE), code 7 (FPE_FLTINV), fault addr 0x5000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGFPE with FPE_FLTINV",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue("Should include root cause", errorOutput.contains("invalid operation"));
  }

  @Test
  public void testDetectsSIGBUSSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 (BUS_ADRALN), fault addr 0x12345678\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_x86_64/generic_x86_64:11'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS signal", errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue("Should include signal line", errorOutput.contains("Fatal signal 7"));
    assertTrue("Should include backtrace", errorOutput.contains("backtrace:"));
  }

  @Test
  public void testDetectsSIGBUSWithUppercase() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with uppercase",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include signal info", errorOutput.contains("SIGBUS"));
  }

  @Test
  public void testDetectsSIGBUSWithLowercase() {
    String logcatOutput =
        "F/libc(12345): fatal signal 7 (sigbus), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS case-insensitively",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testDetectsSignal7() {
    String logcatOutput =
        "F/libc(12345): signal 7 received\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect signal 7", errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include signal info", errorOutput.contains("signal 7"));
  }

  @Test
  public void testDetectsBusErrorKeyword() {
    String logcatOutput =
        "F/libc(12345): Bus error\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect bus error keyword",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include bus error text", errorOutput.contains("Bus error"));
  }

  @Test
  public void testSIGBUSWithBUS_ADRALN() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 (BUS_ADRALN), fault addr 0x12345678 in tid"
            + " 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libnative.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with BUS_ADRALN",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include root cause", errorOutput.contains("invalid address alignment"));
    assertTrue("Should include BUS_ADRALN", errorOutput.contains("BUS_ADRALN"));
  }

  @Test
  public void testSIGBUSWithBUS_ADRERR() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 2 (BUS_ADRERR), fault addr 0xabcdef00\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with BUS_ADRERR",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include root cause", errorOutput.contains("non-existent physical address"));
  }

  @Test
  public void testSIGBUSWithBUS_OBJERR() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 3 (BUS_OBJERR), fault addr 0x1000\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with BUS_OBJERR",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include root cause", errorOutput.contains("object-specific hardware error"));
  }

  @Test
  public void testSIGBUSWithMultipleBacktraceLines() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (accessMemory+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (processData+45)\n"
            + "I/DEBUG(1234):     #02 pc 00009abc  /system/lib64/libart.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with multiple backtrace lines",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include first backtrace line", errorOutput.contains("#00 pc"));
  }

  @Test
  public void testSIGBUSWithNativeLibrary() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234"
            + "  /data/app/com.facebook.app/lib/arm64/libfacebook.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with native library",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include native library", errorOutput.contains("libfacebook.so"));
  }

  @Test
  public void testSIGBUSAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS at end of logcat",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testSIGBUSAtStartOfLogcat() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS at start of logcat",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testSIGBUSWithDebugInfo() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 (BUS_ADRALN), fault addr 0x1234 in tid"
            + " 12345 (MemoryThread)\n"
            + "I/DEBUG(1234): *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***\n"
            + "I/DEBUG(1234): Build fingerprint: 'google/sdk_gphone64_arm64/generic_arm64:12'\n"
            + "I/DEBUG(1234): Revision: '0'\n"
            + "I/DEBUG(1234): ABI: 'arm64'\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with debug info",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include thread name", errorOutput.contains("MemoryThread"));
  }

  @Test
  public void testMultipleSIGBUSOccurrences() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 7 (SIGBUS), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple SIGBUS occurrences",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testSIGBUSWithExceptionAndSignals() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 7 (SIGBUS), code 1 in tid 67890\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect SIGBUS signal", errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testSIGBUSAndOtherSignalsTogether() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 11 (SIGSEGV), code 1 in tid 67890\n"
            + "I/MoreLog(11111): More log\n"
            + "F/libc(33344): Fatal signal 4 (SIGILL), code 1 in tid 33344\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS signal", errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }

  @Test
  public void testSIGBUSWithPcAndSymbol() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00012345  /system/lib64/libc.so (memcpy+256)\n"
            + "I/DEBUG(1234):     #01 pc 00067890 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (copyData+128)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS with symbols",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include pc info", errorOutput.contains("pc 00012345"));
  }

  @Test
  public void testSIGBUSWithJNICall() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 (BUS_ADRALN), fault addr 0x1234\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so"
            + " (Java_com_facebook_jni_NativeClass_readMemory+100)\n"
            + "I/DEBUG(1234):     #01 pc 00005678  /system/lib64/libart.so"
            + " (art_quick_generic_jni_trampoline+152)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS in JNI call",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include JNI method name", errorOutput.contains("Java_com_facebook_jni"));
  }

  @Test
  public void testBusErrorCaseInsensitive() {
    String logcatOutput =
        "F/libc(12345): BUS ERROR\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect bus error case-insensitively",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testSIGBUSWithUserSentSignal() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 0 (SI_USER) in tid 12345\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234  /system/lib64/libc.so\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS sent by user",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include SI_USER", errorOutput.contains("SI_USER"));
  }

  @Test
  public void testSIGBUSWithAlignmentIssue() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 7 (SIGBUS), code 1 (BUS_ADRALN), fault addr 0x12345679\n"
            + "I/DEBUG(1234): backtrace:\n"
            + "I/DEBUG(1234):     #00 pc 00001234 "
            + " /data/app/com.facebook.app/lib/arm64/libnative.so (readInt+42)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect SIGBUS alignment issue",
        errorOutput.contains("SIGBUS signal detected (bus error)"));
    assertTrue("Should include alignment info", errorOutput.contains("invalid address alignment"));
    assertTrue("Should include odd address", errorOutput.contains("0x12345679"));
  }

  @Test
  public void testAllSignalTypesTogether() {
    String logcatOutput =
        "F/libc(12345): Fatal signal 6 (SIGABRT), code -6 in tid 12345\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "F/libc(67890): Fatal signal 11 (SIGSEGV), code 1 in tid 67890\n"
            + "I/MoreLog(11111): More log\n"
            + "F/libc(33344): Fatal signal 4 (SIGILL), code 1 in tid 33344\n"
            + "I/EvenMoreLog(22222): Even more log\n"
            + "F/libc(55566): Fatal signal 8 (SIGFPE), code 1 in tid 55566\n"
            + "I/MoreLog2(33333): More log 2\n"
            + "F/libc(77788): Fatal signal 7 (SIGBUS), code 1 in tid 77788\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue("Should detect SIGABRT signal", errorOutput.contains("SIGABRT signal detected"));
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
    assertTrue(
        "Should detect SIGFPE signal",
        errorOutput.contains("SIGFPE signal detected (floating point exception)"));
    assertTrue(
        "Should detect SIGBUS signal", errorOutput.contains("SIGBUS signal detected (bus error)"));
  }

  @Test
  public void testDetectsClassNotFoundException() {
    String logcatOutput =
        "E/AndroidRuntime(12345): FATAL EXCEPTION: main\n"
            + "E/AndroidRuntime(12345): java.lang.ClassNotFoundException:"
            + " com.nonexistent.FakeClass\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.ClassLoader.loadClass(ClassLoader.java:379)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException",
        errorOutput.contains("Class not found exception detected"));
    assertTrue(
        "Should print crash analysis header", errorOutput.contains("=== AIT CRASH ANALYSIS ==="));
    assertTrue(
        "Should include ClassNotFoundException",
        errorOutput.contains("java.lang.ClassNotFoundException"));
    assertTrue("Should include class name", errorOutput.contains("com.nonexistent.FakeClass"));
  }

  @Test
  public void testDetectsClassNotFoundExceptionWithDetailedMessage() {
    String logcatOutput =
        "E/AndroidRuntime(67890): java.lang.ClassNotFoundException: Didn't find class"
            + " \"com.example.MissingClass\" on path: DexPathList\n"
            + "E/AndroidRuntime(67890): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include detailed message", errorOutput.contains("Didn't find class"));
  }

  @Test
  public void testDetectsClassNotFoundExceptionWithoutStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: com.test.MissingService";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException without stack trace",
        errorOutput.contains("Class not found exception detected"));
    assertTrue(
        "Should include error line", errorOutput.contains("java.lang.ClassNotFoundException"));
  }

  @Test
  public void testClassNotFoundExceptionAtEndOfLogcat() {
    String logcatOutput =
        "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n"
            + "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: com.example.TestClass";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException at end of logcat",
        errorOutput.contains("Class not found exception detected"));
  }

  @Test
  public void testClassNotFoundExceptionAtStartOfLogcat() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: com.example.TestClass\n"
            + "I/SomeLog(1234): Normal log entry\n"
            + "D/AnotherLog(5678): Debug message\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException at start of logcat",
        errorOutput.contains("Class not found exception detected"));
  }

  @Test
  public void testClassNotFoundExceptionWithLongStackTrace() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: com.example.Plugin\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.ClassLoader.loadClass(ClassLoader.java:379)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.ClassLoader.loadClass(ClassLoader.java:312)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.Class.forName(Native Method)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.PluginLoader.load(PluginLoader.java:50)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException with long stack trace",
        errorOutput.contains("Class not found exception detected"));
    assertTrue(
        "Should extract at least one stack frame",
        errorOutput.contains("at dalvik.system.BaseDexClassLoader"));
  }

  @Test
  public void testMultipleClassNotFoundExceptions() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: com.example.FirstClass\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.ClassNotFoundException:"
            + " com.example.SecondClass\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect multiple ClassNotFoundExceptions",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include first class", errorOutput.contains("com.example.FirstClass"));
  }

  @Test
  public void testClassNotFoundExceptionWithOtherExceptions() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.NullPointerException\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.ClassNotFoundException:"
            + " com.example.MissingClass\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect ClassNotFoundException",
        errorOutput.contains("Class not found exception detected"));
  }

  @Test
  public void testClassNotFoundExceptionWithNestedClass() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException:"
            + " com.example.OuterClass$InnerClass\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException for nested class",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include nested class name", errorOutput.contains("OuterClass$InnerClass"));
  }

  @Test
  public void testClassNotFoundExceptionFromClassForName() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: com.facebook.TestClass\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.Class.classForName(Native Method)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.Class.forName(Class.java:453)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.example.Loader.loadClass(Loader.java:100)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException from Class.forName",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include forName in stack", errorOutput.contains("classForName"));
  }

  @Test
  public void testClassNotFoundExceptionWithPackageName() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException:"
            + " com.facebook.app.plugins.example.MyPlugin\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException with long package name",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include package name", errorOutput.contains("com.facebook.app.plugins"));
  }

  @Test
  public void testClassNotFoundExceptionWithDexPath() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException: Didn't find class"
            + " \"com.example.Test\" on path: DexPathList[[zip file"
            + " \"/data/app/com.facebook.app.apk\"]]\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException with DexPathList",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include DexPathList info", errorOutput.contains("DexPathList"));
  }

  @Test
  public void testClassNotFoundExceptionInTestRunner() {
    String logcatOutput =
        "E/TestRunner(12345): java.lang.ClassNotFoundException: com.facebook.test.MyTestClass\n"
            + "E/TestRunner(12345): \tat java.lang.Class.classForName(Native Method)\n"
            + "E/TestRunner(12345): \tat"
            + " android.test.InstrumentationTestRunner.getTargetContext(InstrumentationTestRunner.java:123)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException in test runner",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include test class name", errorOutput.contains("MyTestClass"));
  }

  @Test
  public void testClassNotFoundExceptionWithCausedBy() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.RuntimeException: Unable to instantiate activity\n"
            + "E/AndroidRuntime(12345): \tat"
            + " android.app.ActivityThread.performLaunchActivity(ActivityThread.java:3270)\n"
            + "E/AndroidRuntime(12345): Caused by: java.lang.ClassNotFoundException:"
            + " com.example.MainActivity\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException in Caused by",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include activity name", errorOutput.contains("MainActivity"));
  }

  @Test
  public void testAllExceptionTypesTogether() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed allocation\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.StackOverflowError\n"
            + "I/AnotherLog(11111): Another log\n"
            + "E/AndroidRuntime(11122): java.lang.NullPointerException\n"
            + "I/MoreLog(22222): More log\n"
            + "E/AndroidRuntime(33344): java.lang.ArrayIndexOutOfBoundsException: index=5\n"
            + "I/EvenMoreLog(44444): Even more log\n"
            + "E/AndroidRuntime(55566): java.lang.IllegalStateException: Invalid state\n"
            + "I/MoreLog2(66666): More log 2\n"
            + "E/AndroidRuntime(77788): java.lang.ClassNotFoundException: com.example.Test\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OutOfMemoryError", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should detect StackOverflowError", errorOutput.contains("Stack overflow error detected"));
    assertTrue(
        "Should detect NullPointerException",
        errorOutput.contains("Null pointer exception detected"));
    assertTrue(
        "Should detect ArrayIndexOutOfBoundsException",
        errorOutput.contains("Index out of bounds exception detected"));
    assertTrue(
        "Should detect IllegalStateException",
        errorOutput.contains("Illegal state exception detected"));
    assertTrue(
        "Should detect ClassNotFoundException",
        errorOutput.contains("Class not found exception detected"));
  }

  @Test
  public void testClassNotFoundExceptionWithReflection() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException:"
            + " com.facebook.reflect.ReflectionTarget\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.Class.classForName(Native Method)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.Class.forName(Class.java:453)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " java.lang.Class.forName(Class.java:378)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " com.facebook.reflection.Reflector.instantiate(Reflector.java:42)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException with reflection",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include reflection target", errorOutput.contains("ReflectionTarget"));
  }

  @Test
  public void testClassNotFoundExceptionInServiceLoader() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.ClassNotFoundException:"
            + " com.facebook.service.MyService\n"
            + "E/AndroidRuntime(12345): \tat"
            + " dalvik.system.BaseDexClassLoader.findClass(BaseDexClassLoader.java:207)\n"
            + "E/AndroidRuntime(12345): \tat"
            + " android.app.LoadedApk.makeApplication(LoadedApk.java:1234)\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect ClassNotFoundException in service loader",
        errorOutput.contains("Class not found exception detected"));
    assertTrue("Should include service name", errorOutput.contains("MyService"));
  }

  @Test
  public void testAllExceptionAndSignalTypesTogether() {
    String logcatOutput =
        "E/AndroidRuntime(12345): java.lang.OutOfMemoryError: Failed allocation\n"
            + "I/SomeLog(99999): Some normal log\n"
            + "E/AndroidRuntime(67890): java.lang.ClassNotFoundException: com.example.Test\n"
            + "I/AnotherLog(11111): Another log\n"
            + "F/libc(33344): Fatal signal 11 (SIGSEGV), code 1 in tid 33344\n"
            + "I/MoreLog(44444): More log\n"
            + "F/libc(55566): Fatal signal 4 (SIGILL), code 1 in tid 55566\n";

    crashAnalyzer.analyzeCrashInformation(logcatOutput);

    String errorOutput = errContent.toString();
    assertTrue(
        "Should detect OutOfMemoryError", errorOutput.contains("Out of memory error detected"));
    assertTrue(
        "Should detect ClassNotFoundException",
        errorOutput.contains("Class not found exception detected"));
    assertTrue(
        "Should detect SIGSEGV signal",
        errorOutput.contains("SIGSEGV signal detected (native crash)"));
    assertTrue(
        "Should detect SIGILL signal",
        errorOutput.contains("SIGILL signal detected (illegal instruction)"));
  }
}
