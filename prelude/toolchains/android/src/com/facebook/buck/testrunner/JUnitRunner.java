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
import com.facebook.buck.testresultsoutput.TestResultsOutputEvent;
import com.facebook.buck.testresultsoutput.TestResultsOutputSender;
import com.facebook.buck.testrunner.JavaUtilLoggingHelper.LogHandlers;
import java.io.BufferedWriter;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.io.PrintStream;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.logging.Level;
import junit.framework.TestCase;
import org.jspecify.annotations.Nullable;
import org.junit.Ignore;
import org.junit.Test;
import org.junit.experimental.runners.Enclosed;
import org.junit.internal.builders.AllDefaultPossibilitiesBuilder;
import org.junit.internal.builders.JUnit4Builder;
import org.junit.runner.Computer;
import org.junit.runner.Description;
import org.junit.runner.JUnitCore;
import org.junit.runner.Request;
import org.junit.runner.Result;
import org.junit.runner.RunWith;
import org.junit.runner.Runner;
import org.junit.runner.manipulation.Filter;
import org.junit.runner.notification.Failure;
import org.junit.runner.notification.RunListener;
import org.junit.runners.ParentRunner;
import org.junit.runners.model.RunnerBuilder;

/**
 * Class that runs a set of JUnit tests and writes the results to a directory.
 *
 * <p>IMPORTANT! This class limits itself to types that are available in both the JDK and Android
 * Java API. The objective is to limit the set of files added to the ClassLoader that runs the test,
 * as not to interfere with the results of the test.
 */
public final class JUnitRunner extends BaseRunner {

  static final String JUL_DEBUG_LOGS_HEADER = "====DEBUG LOGS====\n\n";
  static final String JUL_ERROR_LOGS_HEADER = "====ERROR LOGS====\n\n";

  static final String SETUP_CRASH_MESSAGE_PREFIX =
      "The test suite crashed during setup before any test could run, so no per-case results were"
          + " produced. Reported as a fatal run failure (not skipped). Check the suite for an"
          + " exception in class initialization, a @BeforeClass method, or (for Robolectric) the"
          + " sandbox/SDK configuration (e.g. target_sdk_levels). The originating failure"
          + " follows:\n\n";

  private static final String STD_OUT_LOG_LEVEL_PROPERTY = "com.facebook.buck.stdOutLogLevel";
  private static final String STD_ERR_LOG_LEVEL_PROPERTY = "com.facebook.buck.stdErrLogLevel";

  private static final boolean JUNIT_RUN_TESTS_IN_NESTED_CLASSES_ENABLED;

  static {
    String prop = System.getProperty("junit.run_tests_in_nested_classes");
    JUNIT_RUN_TESTS_IN_NESTED_CLASSES_ENABLED = "enabled".equals(prop);
  }

  public JUnitRunner() {}

  @Override
  public void run() throws Throwable {
    setupLogging();
    Level stdOutLogLevel = Level.INFO;
    Level stdErrLogLevel = Level.WARNING;

    String unparsedStdOutLogLevel = System.getProperty(STD_OUT_LOG_LEVEL_PROPERTY);
    String unparsedStdErrLogLevel = System.getProperty(STD_ERR_LOG_LEVEL_PROPERTY);

    if (unparsedStdOutLogLevel != null) {
      stdOutLogLevel = Level.parse(unparsedStdOutLogLevel);
    }

    if (unparsedStdErrLogLevel != null) {
      stdErrLogLevel = Level.parse(unparsedStdErrLogLevel);
    }

    // TPX sets the env var that enables the Standard Output format
    Optional<TestResultsOutputSender> testResultsOutputSender =
        TestResultsOutputSender.fromDefaultEnvName();

    // Per-test coverage: collect per-test-method JaCoCo coverage if enabled
    PerTestJUnitCoverageRunListener perTestCoverageListener = null;
    String perTestCoverageDir = System.getenv("PER_TEST_COVERAGE_DIR");
    if (perTestCoverageDir != null) {
      try {
        perTestCoverageListener = new PerTestJUnitCoverageRunListener(new File(perTestCoverageDir));
      } catch (Exception e) {
        // JaCoCo agent not available — report as infra failure via TPX
        reportRunFailure(
            testResultsOutputSender,
            TestResultsOutputEvent.RunFailureStatus.INFRA_FAILURE,
            "Per-test coverage requested but JaCoCo agent is not available: " + e.getMessage(),
            null);
      }
    }

    // Shared watchdog executor for timeout enforcement (per-test and target-level)
    ScheduledExecutorService watchdogExecutor = Executors.newScheduledThreadPool(1);

    // Proactive timeout: capture thread dump before RE's SIGKILL
    TpxTimeoutBufferManager tpxTimeoutBufferManager = null;
    if (testResultsOutputSender.isPresent()
        && "true".equals(System.getProperty("android.proactive.timeout.enabled"))) {
      tpxTimeoutBufferManager =
          TpxTimeoutBufferManager.create(testResultsOutputSender.get(), watchdogExecutor);
    }

    for (String className : testClassNames) {
      Class<?> testClass = Class.forName(className);
      Class<?>[] testClasses;
      if (JUNIT_RUN_TESTS_IN_NESTED_CLASSES_ENABLED) {
        testClasses = collectTestClasses(testClass);
      } else {
        testClasses = new Class<?>[] {testClass};
      }

      List<TestResult> results = new ArrayList<>();
      RecordingFilter filter = new RecordingFilter();
      if (mightBeATestClass(testClass)) {
        JUnitCore jUnitCore = new JUnitCore();
        Runner suite = new Computer().getSuite(createRunnerBuilder(), testClasses);
        Request request = Request.runner(suite);
        request = request.filterWith(filter);

        JUnitTpxStandardOutputListener tpxListener = null;
        if (testResultsOutputSender.isPresent()) {
          tpxListener = new JUnitTpxStandardOutputListener(testResultsOutputSender.get());
          jUnitCore.addListener(tpxListener);

          if (isRobolectricTest(suite)
              && "true".equals(System.getProperty("android.per.test.timeout.enabled"))) {
            RobolectricTimeoutEnforcingRunListener timeoutListener =
                new RobolectricTimeoutEnforcingRunListener(
                    testResultsOutputSender.get(), watchdogExecutor);
            jUnitCore.addListener(timeoutListener);
          }
        } else {
          jUnitCore.addListener(new TestListener(results, stdOutLogLevel, stdErrLogLevel));
        }

        if (perTestCoverageListener != null) {
          jUnitCore.addListener(perTestCoverageListener);
        }

        jUnitCore.run(request);

        // Report filtered-out tests (e.g., @Ignore) to TPX output so they're not retried
        if (tpxListener != null) {
          for (TestResult filteredTest : filter.filteredOut) {
            if (filteredTest.type == ResultType.DISABLED) {
              String testName =
                  JUnitTpxStandardOutputListener.getFullTestName(
                      filteredTest.testMethodName, filteredTest.testClassName);
              tpxListener.reportOmittedTest(testName, "Test disabled (@Ignore annotation)");
            }
          }
        }

        // On a setup crash the selected cases produce no result and TPX records them as SKIPPED,
        // outside autopilot's FAILED+FATAL filter. Report the run as FATAL rather than synthesizing
        // per-case results for cases that never ran (which would break the TPX/runner contract).
        if (tpxListener != null && !isDryRun) {
          List<String> setupFailureTraces = tpxListener.getUnpairedFailureTraces();
          if (!setupFailureTraces.isEmpty()) {
            String trace = String.join("\n\n", setupFailureTraces);
            reportRunFailure(
                testResultsOutputSender,
                TestResultsOutputEvent.RunFailureStatus.FATAL,
                SETUP_CRASH_MESSAGE_PREFIX + trace,
                trace);
          }
        }
      }
      // Combine the results with the tests we filtered out
      List<TestResult> actualResults = combineResults(results, filter.filteredOut);
      writeResult(className, actualResults);

      // if there is a failure in the list, we should fail entirely.
      results.stream()
          .filter(r -> r.type == ResultType.DRY_RUN && r.failure != null)
          .findFirst()
          .ifPresent(
              result -> {
                System.err.printf(
                    "Failed to list tests for class %s.%n%s",
                    className, stackTraceToString(result.failure));
                System.exit(1);
              });
    }

    if (perTestCoverageListener != null) {
      perTestCoverageListener.close();
      if (perTestCoverageListener.getCoverageError() != null) {
        reportRunFailure(
            testResultsOutputSender,
            TestResultsOutputEvent.RunFailureStatus.INFRA_FAILURE,
            "Per-test coverage collection failed: "
                + perTestCoverageListener.getCoverageError().getMessage(),
            null);
      }
    }

    // All test classes completed normally — mark as completed so shutdown hook is a no-op
    if (tpxTimeoutBufferManager != null) {
      tpxTimeoutBufferManager.markCompleted();
    }
  }

  private static void reportRunFailure(
      Optional<TestResultsOutputSender> sender,
      TestResultsOutputEvent.RunFailureStatus status,
      String message,
      String stacktrace) {
    if (sender.isPresent()) {
      sender.get().sendRunFailure(status, System.currentTimeMillis(), message, stacktrace);
    }
  }

  private void setupLogging() {
    // Check if we have robolectric logging to setup
    // get it as a resource to avoid loading the class
    // and risking any errors in static initialization
    boolean hasRobolectricInClassPath =
        getClass().getClassLoader().getResource("org/robolectric/shadows/ShadowLog.class") != null;

    if (hasRobolectricInClassPath && System.getProperty("robolectric.logging") == null) {
      String configuredTestArtifacts = System.getenv("TEST_RESULT_ARTIFACTS_DIR");
      if (configuredTestArtifacts != null) {
        File testArtifactsDir = new File(configuredTestArtifacts);
        if (!testArtifactsDir.mkdirs() && !testArtifactsDir.exists()) {
          System.err.println(
              "Failed to create test artifacts directory: " + testArtifactsDir.getAbsolutePath());
        }

        String robolectricLogLocation =
            new File(testArtifactsDir, "robolectric-logs.txt").getAbsolutePath();
        System.setProperty("robolectric.logging", robolectricLogLocation);
      }
      String configuredTestAnnotations = System.getenv("TEST_RESULT_ARTIFACT_ANNOTATIONS_DIR");
      if (configuredTestAnnotations != null) {
        File artifactAnnotationsDir = new File(configuredTestAnnotations);
        if (!artifactAnnotationsDir.mkdirs() && !artifactAnnotationsDir.exists()) {
          System.err.println(
              "Failed to create artifact annotations directory: "
                  + artifactAnnotationsDir.getAbsolutePath());
        }

        try (BufferedWriter writer =
            new BufferedWriter(
                new FileWriter(
                    new File(artifactAnnotationsDir, "robolectric-logs.txt.annotation")))) {

          writer.write("{\"type\": {\"generic_text_log\": {}}}");
        } catch (IOException unused) {
          // do nothing
        }
      }
    }
  }

  private static Class<?>[] collectTestClasses(Class<?> testClass) {
    ArrayList<Class<?>> classes = new ArrayList<>();
    for (Class<?> cls : testClass.getDeclaredClasses()) {
      int modifiers = cls.getModifiers();
      if (!Modifier.isStatic(modifiers)) {
        continue;
      }
      if (!Modifier.isPublic(modifiers)) {
        continue;
      }
      if (cls.isAnnotationPresent(RunWith.class)
          && cls.getAnnotation(RunWith.class).value() != Enclosed.class) {
        continue;
      }
      if (mightBeATestClass(cls)) {
        classes.add(cls);
      }
    }
    classes.add(testClass);
    return classes.toArray(new Class<?>[0]);
  }

  /** Guessing whether or not a class is a test class is an imperfect art form. */
  private static boolean mightBeATestClass(Class<?> klass) {
    if (klass.getAnnotation(RunWith.class) != null) {
      return true; // If the class is explicitly marked with @RunWith, it's a test class.
    }
    // Since no RunWith annotation, using standard runner, which requires
    // test classes to be non-abstract/non-interface
    int klassModifiers = klass.getModifiers();
    if (Modifier.isInterface(klassModifiers) || Modifier.isAbstract(klassModifiers)) {
      return false;
    }

    // Classes that extend junit.framework.TestCase are JUnit3-style test classes.
    if (TestCase.class.isAssignableFrom(klass)) {
      return true;
    }

    // Since no RunWith annotation, using standard runner, which requires
    // test classes to have exactly one public constructor (that has no args).
    // Classes may have (non-public) constructors (with or without args).
    boolean foundPublicNoArgConstructor = false;
    for (Constructor<?> c : klass.getConstructors()) {
      if (Modifier.isPublic(c.getModifiers())) {
        if (c.getParameterCount() != 0) {
          return false;
        }
        foundPublicNoArgConstructor = true;
      }
    }
    if (!foundPublicNoArgConstructor) {
      return false;
    }
    // If the class has a JUnit4 @Test-annotated method, it's a test class.
    boolean hasAtLeastOneTest = false;
    for (Method m : klass.getMethods()) {
      if (Modifier.isPublic(m.getModifiers())
          && m.getParameters().length == 0
          && m.getAnnotation(Test.class) != null) {
        hasAtLeastOneTest = true;
        break;
      }
    }
    return hasAtLeastOneTest;
  }

  /**
   * This method filters a list of test results prior to writing results to a file. null is returned
   * to indicate "don't write anything", which is different to writing a file containing 0 results.
   *
   * <p>JUnit handles classes-without-tests in different ways. If you are not using the
   * org.junit.runner.Request.filterWith facility then JUnit ignores classes-without-tests. However,
   * if you are using a filter then a class-without-tests will cause a NoTestsRemainException to be
   * thrown, which is propagated back as an error.
   */
  static List<TestResult> combineResults(
      List<TestResult> results, List<TestResult> filteredResults) {
    List<TestResult> combined = new ArrayList<>(filteredResults);
    if (!isSingleResultCausedByNoTestsRemainException(results)) {
      combined.addAll(results);
    }
    return combined;
  }

  /**
   * JUnit doesn't normally consider encountering a testless class an error. However, when using
   * org.junit.runner.manipulation.Filter, testless classes *are* considered an error, throwing
   * org.junit.runner.manipulation.NoTestsRemainException.
   *
   * <p>If we are using test-selectors then it's possible we will run a test class but never run any
   * of its test methods, because they'd all get filtered out. When this happens, the results will
   * contain a single failure containing the error from the NoTestsRemainException.
   *
   * <p>However, there is another reason why the test class may have a single failure -- if the
   * class fails to instantiate, then it doesn't get far enough to detect whether or not there were
   * any tests. In that case, JUnit4 returns a single failure result with the testMethodName set to
   * "initializationError".
   *
   * <p>(NB: we can't decide at the class level whether we need to run a test class or not; we can
   * only run the test class and all its test methods and handle the erroneous exception JUnit
   * throws if no test-methods were actually run.)
   */
  private static boolean isSingleResultCausedByNoTestsRemainException(List<TestResult> results) {
    if (results.size() != 1) {
      return false;
    }
    TestResult singleResult = results.get(0);
    return !singleResult.isSuccess()
        && "initializationError".equals(singleResult.testMethodName)
        && "org.junit.runner.manipulation.Filter".equals(singleResult.testClassName);
  }

  /**
   * Creates an {@link AllDefaultPossibilitiesBuilder} that returns our custom {@link
   * BuckBlockJUnit4ClassRunner} when a {@link JUnit4Builder} is requested. This ensures that JUnit
   * 4 tests are executed using our runner whereas other types of tests are run with whatever JUnit
   * thinks is best.
   */
  private RunnerBuilder createRunnerBuilder() {
    JUnit4Builder jUnit4RunnerBuilder =
        new JUnit4Builder() {
          @Override
          public Runner runnerForClass(Class<?> testClass) throws Throwable {
            return new BuckBlockJUnit4ClassRunner(testClass, defaultTestTimeoutMillis);
          }
        };

    return new AllDefaultPossibilitiesBuilder() {
      @Override
      protected JUnit4Builder junit4Builder() {
        return jUnit4RunnerBuilder;
      }
    };
  }

  /** Checks if a test class is a Robolectric test by examining its runner tree. */
  private boolean isRobolectricTest(Runner runner) {
    try {
      Class<?> robolectricTestRunner = Class.forName("org.robolectric.RobolectricTestRunner");
      Class<?> suiteRunner = Class.forName("org.junit.runners.Suite");
      return isRunnerOfTypeOrSuiteOfType(runner, robolectricTestRunner, suiteRunner);
    } catch (ClassNotFoundException e) {
      return false;
    }
  }

  static boolean isRunnerOfTypeOrSuiteOfType(
      Runner runner, Class<?> runnerType, Class<?> suiteType) {
    if (runnerType.isAssignableFrom(runner.getClass())) {
      return true;
    }

    if (!suiteType.isAssignableFrom(runner.getClass())) {
      return false;
    }

    try {
      Method getChildrenMethod = ParentRunner.class.getDeclaredMethod("getChildren");
      getChildrenMethod.setAccessible(true);
      List<?> children = (List<?>) getChildrenMethod.invoke(runner);

      if (children.isEmpty()) {
        return false;
      }

      for (Object child : children) {
        if (!(child instanceof Runner)
            || !isRunnerOfTypeOrSuiteOfType((Runner) child, runnerType, suiteType)) {
          return false;
        }
      }
      return true;
    } catch (ReflectiveOperationException e) {
      return false;
    }
  }

  /**
   * Creates RunListener that will prepare individual result for each test and store it to results
   * list afterwards.
   */
  private class TestListener extends RunListener {
    private final List<TestResult> results;
    private final Level stdErrLogLevel;
    private final Level stdOutLogLevel;
    @Nullable private PrintStream originalOut, originalErr, stdOutStream, stdErrStream;
    @Nullable private ByteArrayOutputStream rawStdOutBytes, rawStdErrBytes;
    @Nullable private ByteArrayOutputStream julLogBytes, julErrLogBytes;
    @Nullable private LogHandlers logHandlers;
    @Nullable private Result result;
    @Nullable private RunListener resultListener;
    @Nullable private Failure assumptionFailure;

    // To help give a reasonable (though imprecise) guess at the runtime for unpaired failures
    private final long startTime = System.currentTimeMillis();

    TestListener(List<TestResult> results, Level stdOutLogLevel, Level stdErrLogLevel) {
      this.results = results;
      this.stdOutLogLevel = stdOutLogLevel;
      this.stdErrLogLevel = stdErrLogLevel;
    }

    @Override
    public void testStarted(Description description) throws Exception {
      // Create an intermediate stdout/stderr to capture any debugging statements (usually in the
      // form of System.out.println) the developer is using to debug the test.
      originalOut = System.out;
      originalErr = System.err;
      rawStdOutBytes = new ByteArrayOutputStream();
      rawStdErrBytes = new ByteArrayOutputStream();
      julLogBytes = new ByteArrayOutputStream();
      julErrLogBytes = new ByteArrayOutputStream();
      stdOutStream = new PrintStream(rawStdOutBytes, true /* autoFlush */, ENCODING);
      stdErrStream = new PrintStream(rawStdErrBytes, true /* autoFlush */, ENCODING);
      System.setOut(stdOutStream);
      System.setErr(stdErrStream);

      // Set up logging handlers
      logHandlers =
          JavaUtilLoggingHelper.setupLogging(
              julLogBytes, julErrLogBytes, stdOutLogLevel, stdErrLogLevel);

      // Prepare single-test result.
      result = new Result();
      resultListener = result.createListener();
      resultListener.testRunStarted(description);
      resultListener.testStarted(description);
    }

    @Override
    public void testFinished(Description description) throws Exception {
      // Shutdown single-test result.
      resultListener.testFinished(description);
      resultListener.testRunFinished(result);
      resultListener = null;

      // Restore the original stdout/stderr.
      System.setOut(originalOut);
      System.setErr(originalErr);

      // Clean up logging handlers
      JavaUtilLoggingHelper.cleanupLogging(logHandlers);
      logHandlers = null;

      // Get the stdout/stderr written during the test as strings.
      stdOutStream.flush();
      stdErrStream.flush();

      int numFailures = result.getFailureCount();
      String className = description.getClassName();
      String methodName = description.getMethodName();

      Failure failure;
      ResultType type;
      if (assumptionFailure != null) {
        failure = assumptionFailure;
        type = ResultType.ASSUMPTION_VIOLATION;
        // Clear the assumption-failure field before the next test result appears.
        assumptionFailure = null;
      } else if (isDryRun) {
        if ("org.junit.runner.manipulation.Filter".equals(className)
            && "initializationError".equals(methodName)) {
          return; // don't record errors from failed class initialization during dry run
        }
        failure = numFailures == 0 ? null : result.getFailures().get(0);
        if (failure != null
            && stackTraceToString(failure.getException()).contains("No runnable methods")
            && "initializationError".equals(methodName)) {
          return; // don't record errors if there is no runnable
        }
        type = ResultType.DRY_RUN;
      } else if (numFailures == 0) {
        failure = null;
        type = ResultType.SUCCESS;
      } else {
        failure = result.getFailures().get(0);
        type = ResultType.FAILURE;
      }

      StringBuilder stdOut = new StringBuilder();
      stdOut.append(rawStdOutBytes.toString(ENCODING));
      if (type == ResultType.FAILURE && julLogBytes.size() > 0) {
        stdOut.append('\n');
        stdOut.append(JUL_DEBUG_LOGS_HEADER);
        stdOut.append(julLogBytes.toString(ENCODING));
      }
      StringBuilder stdErr = new StringBuilder();
      stdErr.append(rawStdErrBytes.toString(ENCODING));
      if (type == ResultType.FAILURE && julErrLogBytes.size() > 0) {
        stdErr.append('\n');
        stdErr.append(JUL_ERROR_LOGS_HEADER);
        stdErr.append(julErrLogBytes.toString(ENCODING));
      }

      results.add(
          new TestResult(
              className,
              methodName,
              result.getRunTime(),
              type,
              failure == null ? null : failure.getException(),
              stdOut.length() == 0 ? null : stdOut.toString(),
              stdErr.length() == 0 ? null : stdErr.toString()));
    }

    @Override
    public void testRunFinished(Result runResult) {
      if (resultListener != null) {
        // testStarted was called for latest test, but not the testFinished
        // report all failures as unbounded
        for (Failure failure : result.getFailures()) {
          recordUnpairedResult(failure, ResultType.FAILURE);
        }
      }
    }

    /**
     * The regular listener we created from the singular result, in this class, will not by default
     * treat assumption failures as regular failures, and will not store them. As a consequence, we
     * store them ourselves!
     *
     * <p>We store the assumption-failure in a temporary field, which we'll make sure we clear each
     * time we write results.
     */
    @Override
    public void testAssumptionFailure(Failure failure) {
      assumptionFailure = failure;
      if (resultListener == null) {
        recordUnpairedResult(failure, ResultType.ASSUMPTION_VIOLATION);
      } else {
        // Left in only to help catch future bugs -- right now this does nothing.
        resultListener.testAssumptionFailure(failure);
      }
    }

    @Override
    public void testFailure(Failure failure) throws Exception {
      if (resultListener == null) {
        recordUnpairedResult(failure, ResultType.FAILURE);
      } else {
        resultListener.testFailure(failure);
      }
    }

    @Override
    public void testIgnored(Description description) throws Exception {
      if (resultListener != null) {
        resultListener.testIgnored(description);
      }
    }

    /**
     * It's possible to encounter a Failure/Skip before we've started any tests (and therefore
     * before testStarted() has been called). The known example is a @BeforeClass that throws an
     * exception, but there may be others.
     *
     * <p>Recording these unexpected failures helps us propagate failures back up to the "buck test"
     * process.
     */
    private void recordUnpairedResult(Failure failure, ResultType resultType) {
      long runtime = System.currentTimeMillis() - startTime;
      Description description = failure.getDescription();
      results.add(
          new TestResult(
              description.getClassName(),
              description.getMethodName(),
              runtime,
              resultType,
              failure.getException(),
              null,
              null));
    }
  }

  /** A JUnit Filter that records the tests it filters out. */
  private class RecordingFilter extends Filter {
    static final String FILTER_DESCRIPTION = "TestSelectorList-filter";

    List<TestResult> filteredOut = new ArrayList<>();

    @Override
    public boolean shouldRun(Description description) {
      String methodName = description.getMethodName();
      if (methodName == null) {
        // JUnit will give us an org.junit.runner.Description like this for the
        // test class
        // itself.  It's easier for our filtering to make decisions just at the method level,
        // however, so just always return true here.
        return true;
      }
      String className = description.getClassName();
      TestDescription testDescription = new TestDescription(className, methodName);
      TestSelector matchingSelector = testSelectorList.findSelector(testDescription);
      if (!matchingSelector.isInclusive()) {
        if (shouldExplainTestSelectors) {
          String reason = "Excluded by filter: " + matchingSelector.getExplanation();
          filteredOut.add(TestResult.forExcluded(className, methodName, reason));
        }
        return false;
      }
      if (description.getAnnotation(Ignore.class) != null) {
        filteredOut.add(TestResult.forDisabled(className, methodName));
        return false;
      }
      if (isDryRun) {
        filteredOut.add(TestResult.forDryRun(className, methodName));
        return false;
      }
      return true;
    }

    @Override
    public String describe() {
      return FILTER_DESCRIPTION;
    }
  }
}
