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

import com.facebook.buck.test.result.type.ResultType;
import com.facebook.buck.test.selectors.TestDescription;
import com.facebook.buck.test.selectors.TestSelector;
import com.facebook.infer.annotation.Nullsafe;
import java.io.ByteArrayOutputStream;
import java.io.PrintStream;
import java.io.UnsupportedEncodingException;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.stream.Collectors;
import java.util.stream.Stream;
import org.jspecify.annotations.Nullable;
import org.testng.IAnnotationTransformer;
import org.testng.IClass;
import org.testng.IConfigurationListener;
import org.testng.IReporter;
import org.testng.ITestContext;
import org.testng.ITestListener;
import org.testng.ITestResult;
import org.testng.TestNG;
import org.testng.annotations.AfterClass;
import org.testng.annotations.AfterGroups;
import org.testng.annotations.AfterMethod;
import org.testng.annotations.AfterSuite;
import org.testng.annotations.AfterTest;
import org.testng.annotations.BeforeClass;
import org.testng.annotations.BeforeGroups;
import org.testng.annotations.BeforeMethod;
import org.testng.annotations.BeforeSuite;
import org.testng.annotations.BeforeTest;
import org.testng.annotations.Factory;
import org.testng.annotations.Guice;
import org.testng.annotations.ITestAnnotation;
import org.testng.annotations.Test;
import org.testng.reporters.EmailableReporter;
import org.testng.reporters.FailedReporter;
import org.testng.reporters.JUnitReportReporter;
import org.testng.reporters.SuiteHTMLReporter;
import org.testng.reporters.XMLReporter;

/** Class that runs a set of TestNG tests and writes the results to a directory. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public final class TestNGRunner extends BaseRunner {
  private static final boolean TESTNG_ALT_TEST_NAME_GEN_ENABLED;

  static {
    String prop = System.getProperty("testng.alt_test_name_gen");
    TESTNG_ALT_TEST_NAME_GEN_ENABLED = "enabled".equals(prop);
  }

  @Override
  public void run() throws Throwable {
    for (String className : testClassNames) {

      Class<?> testClass = Class.forName(className);

      List<TestResult> results;
      if (!mightBeATestClass(testClass)) {
        results = Collections.emptyList();
      } else {
        results = new ArrayList<>();
        TestNG testng = new TestNG();
        testng.setUseDefaultListeners(false);
        testng.addListener(new FilteringAnnotationTransformer(results, testClass.getName()));
        testng.setTestClasses(new Class<?>[] {testClass});
        testng.addListener(new TestListener(results));
        // use default TestNG reporters ...
        testng.addListener(new SuiteHTMLReporter());
        testng.addListener((IReporter) new FailedReporter());
        testng.addListener(new XMLReporter());
        testng.addListener(new EmailableReporter());
        // ... except this replaces JUnitReportReporter ...
        testng.addListener(new JUnitReportReporterWithMethodParameters());
        // ... and we can't access TestNG verbosity, so we remove VerboseReporter
        testng.run();
      }

      writeResult(className, results);
    }
  }

  /** Guessing whether or not a class is a test class is an imperfect art form. */
  private boolean mightBeATestClass(Class<?> klass) {
    int klassModifiers = klass.getModifiers();
    // Test classes must be public, non-abstract, non-interface
    if (!Modifier.isPublic(klassModifiers)
        || Modifier.isInterface(klassModifiers)
        || Modifier.isAbstract(klassModifiers)) {
      return false;
    }
    // Test classes must either have a public, no-arg constructor, or have a constructor that
    // initializes using dependency injection, via the org.testng.annotations.Guice annotation on
    // the class and the com.google.inject.Inject or javax.inject.Inject annotation on the
    // constructor.
    boolean foundPublicNoArgConstructor = false;
    boolean foundInjectedConstructor = false;
    boolean hasGuiceAnnotation = klass.getAnnotationsByType(Guice.class).length > 0;
    for (Constructor<?> c : klass.getConstructors()) {
      if (Modifier.isPublic(c.getModifiers())) {
        if (c.getParameterCount() == 0) {
          foundPublicNoArgConstructor = true;
        }
        if (hasGuiceAnnotation
            && (c.getAnnotationsByType(com.google.inject.Inject.class).length > 0
                || c.getAnnotationsByType(javax.inject.Inject.class).length > 0)) {
          foundInjectedConstructor = true;
        }
      }
    }
    if (!foundPublicNoArgConstructor && !foundInjectedConstructor) {
      return false;
    }
    // Test classes must have at least one public test method (or something that generates tests)
    boolean hasAtLeastOneTestMethod = false;
    for (Method m : klass.getMethods()) {
      if (Modifier.isPublic(m.getModifiers()) && m.getAnnotation(Test.class) != null) {
        hasAtLeastOneTestMethod = true;
      }
      if (Modifier.isPublic(m.getModifiers()) && m.getAnnotation(Factory.class) != null) {
        hasAtLeastOneTestMethod = true; // technically, not *quite* true, but close enough
      }
    }
    return hasAtLeastOneTestMethod;
  }

  private static String getTestMethodNameWithParameters(ITestResult iTestResult) {
    return getTestMethodNameWithParameters(iTestResult, -1);
  }

  /** Compute the "full name" of a test method, including its parameters, if any. */
  private static String getTestMethodNameWithParameters(ITestResult iTestResult, int count) {
    Object[] parameters = iTestResult.getParameters();
    String name = iTestResult.getName();

    if (parameters == null || parameters.length == 0) {
      return name;
    }

    StringBuilder builder = new StringBuilder(name);

    if (TESTNG_ALT_TEST_NAME_GEN_ENABLED && count >= 0) {
      builder.append(" [").append(count).append("]");
    } else {
      builder
          .append(" (")
          .append(
              Arrays.stream(parameters)
                  .map(
                      parameter -> {
                        try {
                          return String.valueOf(parameter);
                        } catch (Exception e) {
                          return "Unstringable object";
                        }
                      })
                  .collect(Collectors.joining(", ")))
          .append(")");
    }

    return builder.toString();
  }

  public class FilteringAnnotationTransformer implements IAnnotationTransformer {
    final List<TestResult> results;
    private String testClassName;

    FilteringAnnotationTransformer(List<TestResult> results, String testClassName) {
      this.results = results;
      this.testClassName = testClassName;
    }

    @Override
    @SuppressWarnings("rawtypes")
    public void transform(
        ITestAnnotation annotation,
        Class testClass,
        Constructor testConstructor,
        Method testMethod) {
      if (testMethod == null) {
        return;
      }
      if (hasConfigurationAnnotation(testMethod)) {
        transformConfiguration(annotation);
      } else {
        transformTest(annotation, testMethod);
      }
    }

    private void transformConfiguration(ITestAnnotation annotation) {
      if (isDryRun) {
        annotation.setEnabled(false);
      }
    }

    private void transformTest(ITestAnnotation annotation, Method testMethod) {
      String methodName = testMethod.getName();
      TestDescription description = new TestDescription(testClassName, methodName);
      TestSelector matchingSelector = testSelectorList.findSelector(description);
      if (!matchingSelector.isInclusive()) {
        // For tests that have been filtered out, record it now and don't run it
        if (shouldExplainTestSelectors) {
          String reason = "Excluded by filter: " + matchingSelector.getExplanation();
          results.add(TestResult.forExcluded(testClassName, methodName, reason));
        }
        annotation.setEnabled(false);
      } else if (!annotation.getEnabled()) {
        // on a dry run, have to record it now -- since it doesn't run, listener can't do it
        results.add(TestResult.forDisabled(testClassName, methodName));
      } else if (isDryRun) {
        // on a dry run, record it now and don't run it
        results.add(TestResult.forDryRun(testClassName, methodName));
        annotation.setEnabled(false);
      }
    }

    private boolean hasConfigurationAnnotation(Method testMethod) {
      return Stream.of(
              BeforeSuite.class,
              AfterSuite.class,
              BeforeTest.class,
              AfterTest.class,
              BeforeGroups.class,
              AfterGroups.class,
              BeforeClass.class,
              AfterClass.class,
              BeforeMethod.class,
              AfterMethod.class)
          .anyMatch(testMethod::isAnnotationPresent);
    }
  }

  private static class TestListener implements ITestListener, IConfigurationListener {
    private final List<TestResult> results;
    private boolean mustRestoreStdoutAndStderr;
    @Nullable private PrintStream originalOut, originalErr, stdOutStream, stdErrStream;
    @Nullable private ByteArrayOutputStream rawStdOutBytes, rawStdErrBytes;
    private Map<IClass, Throwable> failedConfigurationTestClasses = new HashMap<>();
    private Map<String, Integer> testNameCounts = new HashMap<>();

    public TestListener(List<TestResult> results) {
      this.results = results;
    }

    @Override
    public void onTestStart(ITestResult result) {}

    @Override
    public void onTestSuccess(ITestResult result) {
      recordResult(result, ResultType.SUCCESS, result.getThrowable());
    }

    @Override
    public void onTestSkipped(ITestResult result) {
      @Nullable Throwable throwable = failedConfigurationTestClasses.get(result.getTestClass());
      if (throwable == null) {
        recordResult(result, ResultType.ASSUMPTION_VIOLATION, result.getThrowable());
      } else {
        recordResult(result, ResultType.FAILURE, throwable);
      }
    }

    @Override
    public void onTestFailure(ITestResult result) {
      recordResult(result, ResultType.FAILURE, result.getThrowable());
    }

    @Override
    public void onTestFailedButWithinSuccessPercentage(ITestResult result) {
      recordResult(result, ResultType.FAILURE, result.getThrowable());
    }

    @Override
    public void onStart(ITestContext context) {
      // Create an intermediate stdout/stderr to capture any debugging statements (usually in the
      // form of System.out.println) the developer is using to debug the test.
      originalOut = System.out;
      originalErr = System.err;
      rawStdOutBytes = new ByteArrayOutputStream();
      rawStdErrBytes = new ByteArrayOutputStream();
      stdOutStream = streamToPrintStream(rawStdOutBytes, System.out);
      stdErrStream = streamToPrintStream(rawStdErrBytes, System.err);
      System.setOut(stdOutStream);
      System.setErr(stdErrStream);
      mustRestoreStdoutAndStderr = true;
    }

    @Override
    public void onFinish(ITestContext context) {
      if (mustRestoreStdoutAndStderr) {
        // Restore the original stdout/stderr.
        System.setOut(originalOut);
        System.setErr(originalErr);

        // Get the stdout/stderr written during the test as strings.
        Objects.requireNonNull(stdOutStream).flush();
        Objects.requireNonNull(stdErrStream).flush();
        mustRestoreStdoutAndStderr = false;
      }
    }

    private void recordResult(ITestResult result, ResultType type, @Nullable Throwable failure) {
      String stdOut = streamToString(Objects.requireNonNull(rawStdOutBytes));
      String stdErr = streamToString(Objects.requireNonNull(rawStdErrBytes));

      String className = result.getTestClass().getName();
      String name = result.getName();
      testNameCounts.putIfAbsent(name, 0);
      // Get index by each test method
      int count = Objects.requireNonNull(testNameCounts.get(name));
      String methodName = getTestMethodNameWithParameters(result, count);
      testNameCounts.put(name, count + 1);

      long runTimeMillis = result.getEndMillis() - result.getStartMillis();
      results.add(
          new TestResult(className, methodName, runTimeMillis, type, failure, stdOut, stdErr));
    }

    @Nullable
    private String streamToString(ByteArrayOutputStream str) {
      try {
        return str.size() == 0 ? null : str.toString(ENCODING);
      } catch (UnsupportedEncodingException e) {
        return null;
      }
    }

    private PrintStream streamToPrintStream(ByteArrayOutputStream str, PrintStream fallback) {
      try {
        return new PrintStream(str, true /* autoFlush */, ENCODING);
      } catch (UnsupportedEncodingException e) {
        return fallback;
      }
    }

    @Override
    public void onConfigurationSuccess(ITestResult iTestResult) {}

    @Override
    public void onConfigurationFailure(ITestResult iTestResult) {
      failedConfigurationTestClasses.put(iTestResult.getTestClass(), iTestResult.getThrowable());
    }

    @Override
    public void onConfigurationSkip(ITestResult iTestResult) {}
  }

  private static class JUnitReportReporterWithMethodParameters extends JUnitReportReporter {
    @Override
    public String getTestName(ITestResult result) {
      return getTestMethodNameWithParameters(result);
    }
  }
}
