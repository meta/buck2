/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.testrunner

import com.android.ddmlib.testrunner.ITestRunListener
import com.android.ddmlib.testrunner.TestIdentifier
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit

/**
 * An ITestRunListener that enforces timeouts for instrumentation tests and reports failures to an
 * XML listener.
 *
 * This listener monitors test execution and forcibly terminates tests that exceed the configured
 * timeout period. When a test times out, it notifies the XML listener via testFailed/testEnded
 * callbacks and then calls System.exit(1) to terminate the JVM.
 *
 * The timeout is configured via environment variables defined in [InstrumentationTestRunner].
 */
class InstrumentationTimeoutEnforcingRunListener(private val xmlListener: ITestRunListener) :
    ITestRunListener {

  companion object {
    /** Default timeout in milliseconds (60 seconds). */
    private const val DEFAULT_TIMEOUT_MS = 60 * 1000L
  }

  private val watchdogExecutor = Executors.newSingleThreadScheduledExecutor()
  private val activeTimeouts: MutableMap<TestIdentifier, ScheduledFuture<*>> = ConcurrentHashMap()

  override fun testStarted(test: TestIdentifier) {
    val timeoutMs = getTimeout()

    // Schedule timeout task
    val timeoutTask =
        watchdogExecutor.schedule(
            { handleTimeout(test, timeoutMs) },
            timeoutMs,
            TimeUnit.MILLISECONDS,
        )

    activeTimeouts[test] = timeoutTask
  }

  override fun testEnded(test: TestIdentifier, testMetrics: MutableMap<String, String>) {
    cancelExecution(test)
  }

  override fun testFailed(test: TestIdentifier, trace: String) {
    cancelExecution(test)
  }

  override fun testAssumptionFailure(test: TestIdentifier, trace: String) {
    cancelExecution(test)
  }

  override fun testIgnored(test: TestIdentifier) {
    cancelExecution(test)
  }

  override fun testRunStarted(runName: String, testCount: Int) {}

  override fun testRunFailed(errorMessage: String) {
    shutdown()
  }

  override fun testRunEnded(elapsedTime: Long, runMetrics: MutableMap<String, String>) {
    shutdown()
  }

  override fun testRunStopped(elapsedTime: Long) {
    shutdown()
  }

  private fun cancelExecution(test: TestIdentifier) {
    activeTimeouts.remove(test)?.cancel(false)
  }

  private fun getTimeout(): Long {
    val buckMultiplierEnv = System.getenv("ANDROID_PER_TEST_TIMEOUT_MULTIPLIER")
    val multiplier = buckMultiplierEnv?.toIntOrNull()?.coerceAtLeast(1) ?: 1

    return DEFAULT_TIMEOUT_MS * multiplier
  }

  /** Handles a test timeout by notifying the XML listener and forcibly terminating the JVM. */
  private fun handleTimeout(test: TestIdentifier, timeoutMs: Long) {
    activeTimeouts.remove(test) ?: return

    val message =
        "Test timed out after ${timeoutMs}ms. " +
            "If your test needs to run longer than ${timeoutMs / 1000} seconds, add the tpx long_running or glacial tag in the labels section of the BUCK target. " +
            "See https://fb.workplace.com/groups/android.testing.fyi/permalink/2679204925789466/ for more details"
    System.err.println(message)

    // Notify the XML listener about the failure
    try {
      xmlListener.testFailed(test, message)
      xmlListener.testEnded(test, mutableMapOf())
      xmlListener.testRunEnded(0, mutableMapOf())
    } catch (e: Exception) {
      System.err.println("Failed to notify XML listener: ${e.message}")
    }

    printThreadDump()

    // This is necessary because instrumentation tests may not respond to thread interruption
    System.exit(1)
  }

  /** Prints a thread dump to help diagnose what the test was doing when it timed out. */
  private fun printThreadDump() {
    System.err.println("\n=== Thread Dump ===")
    Thread.getAllStackTraces().forEach { (thread, stackTrace) ->
      System.err.println("Thread: ${thread.name} (${thread.state})")
      stackTrace.forEach { element -> System.err.println("  at $element") }
      System.err.println()
    }
  }

  /** Shuts down the watchdog executor when testing is complete. */
  private fun shutdown() {
    watchdogExecutor.shutdownNow()
  }
}
