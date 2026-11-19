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
import static org.junit.Assert.assertTrue;
import static org.junit.Assert.fail;

import com.google.common.collect.ImmutableSet;
import com.google.common.collect.Iterables;
import java.util.HashSet;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;
import org.junit.Test;
import org.junit.runner.Computer;
import org.junit.runner.JUnitCore;
import org.junit.runner.Request;
import org.junit.runner.Result;
import org.junit.runner.Runner;
import org.junit.runner.notification.Failure;
import org.junit.runner.notification.RunListener;
import org.junit.runners.model.InitializationError;
import org.junit.runners.model.RunnerBuilder;

public class TimeoutTest {

  /**
   * Verify that we avoid the issue where adding a timeout causes tests to be run on different
   * thread to the one that they were created on.
   *
   * <p>https://github.com/junit-team/junit/issues/686
   */
  @Test
  public void testsShouldRunOnTheThreadTheyAreCreatedOn() throws InitializationError {
    ThreadGuardedTest.isBeingUsedForTimeoutTest.set(true);
    try {
      doTestUsingThreadGuardedTestClass();
    } finally {
      ThreadGuardedTest.isBeingUsedForTimeoutTest.set(false);
    }
  }

  private void doTestUsingThreadGuardedTestClass() throws InitializationError {
    Class<?> testClass = ThreadGuardedTest.class;

    RunnerBuilder builder =
        new RunnerBuilder() {
          @Override
          public Runner runnerForClass(Class<?> clazz) throws Throwable {
            return new BuckBlockJUnit4ClassRunner(clazz, /* defaultTestTimeoutMillis */ 500);
          }
        };
    Runner suite = new Computer().getSuite(builder, new Class<?>[] {testClass});
    Request request = Request.runner(suite);

    Set<Result> results = new HashSet<>();
    JUnitCore core = new JUnitCore();
    core.addListener(
        new RunListener() {
          @Override
          public void testRunFinished(Result result) {
            results.add(result);
          }
        });
    core.run(request);

    Result result = Iterables.getOnlyElement(results);
    assertEquals(3, result.getRunCount());
    assertEquals(2, result.getFailureCount());

    // The order in which the tests were run doesn't matter. What matters is that we see our
    // expected messages.
    Set<String> messages =
        result.getFailures().stream()
            .map(Failure::getMessage)
            .collect(ImmutableSet.toImmutableSet());
    assertEquals(
        "Should contain explicit call to fail() from failingTestsAreReported() and "
            + "the timeout message from testsMayTimeOut().",
        ImmutableSet.of(
            "This is expected", "test testsMayTimeOut timed out after 500 milliseconds"),
        messages);
  }

  public static class ThreadGuardedTest {
    public static final AtomicBoolean isBeingUsedForTimeoutTest = new AtomicBoolean(false);

    private final long creatorThreadId = Thread.currentThread().getId();

    @Test
    public void verifyTestRunsOnCreatorThread() {
      assertTrue(isBeingUsedForTimeoutTest.get());
      assertEquals(creatorThreadId, Thread.currentThread().getId());
    }

    @Test
    public void testsMayTimeOut() throws InterruptedException {
      assertTrue(isBeingUsedForTimeoutTest.get());
      Thread.sleep(5000);
    }

    @Test
    public void failingTestsAreReported() {
      assertTrue(isBeingUsedForTimeoutTest.get());
      fail("This is expected");
    }
  }
}
