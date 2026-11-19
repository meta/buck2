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

import com.facebook.infer.annotation.Nullsafe;
import java.util.Map;
import java.util.concurrent.Callable;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Future;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import org.junit.runners.model.Statement;

@Nullsafe(Nullsafe.Mode.LOCAL)
class SameThreadFailOnTimeout extends Statement {
  private final ExecutorService executor;
  private final long timeout;
  private final String testName;
  private final Callable<Throwable> callable;

  public SameThreadFailOnTimeout(
      ExecutorService executor, long timeout, String testName, Statement next) {
    this.executor = executor;
    this.timeout = timeout;
    this.testName = testName;
    this.callable =
        () -> {
          try {
            next.evaluate();
            return null;
          } catch (Throwable throwable) {
            return throwable;
          }
        };
  }

  @Override
  public void evaluate() throws Throwable {
    Future<Throwable> submitted = executor.submit(callable);
    try {
      Throwable result = submitted.get(timeout, TimeUnit.MILLISECONDS);
      if (result == null) {
        return;
      }
      if (result instanceof TimeoutException) {
        throw new Exception("A timeout occurred inside of the test case", result);
      }
      throw result;
    } catch (TimeoutException e) {
      System.err.printf("Dumping threads for timed-out test %s:%n", testName);
      for (Map.Entry<Thread, StackTraceElement[]> t : Thread.getAllStackTraces().entrySet()) {
        Thread thread = t.getKey();
        System.err.printf("\"%s\" #%d%n", thread.getName(), thread.getId());
        System.err.printf("\tjava.lang.Thread.State: %s%n", thread.getState());
        for (StackTraceElement element : t.getValue()) {
          System.err.printf("\t\t at %s%n", element);
        }
      }

      submitted.cancel(true);

      // The default timeout doesn't indicate which test was running.
      String message = String.format("test %s timed out after %d milliseconds", testName, timeout);

      throw new Exception(message);
    }
  }
}
