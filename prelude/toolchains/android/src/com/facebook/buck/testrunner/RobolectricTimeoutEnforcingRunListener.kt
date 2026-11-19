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

import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.TestStatus
import com.facebook.buck.testresultsoutput.TestResultsOutputSender
import java.util.Optional
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import org.junit.runner.Description
import org.junit.runner.notification.Failure
import org.junit.runner.notification.RunListener

/**
 * A RunListener that enforces timeouts for Robolectric tests.
 *
 * This listener monitors test execution and forcibly terminates tests that exceed the configured
 * timeout period. It uses System.exit(1) to ensure timeout enforcement
 *
 * Note: This listener assumes it's being used only for Robolectric tests with TPX output enabled.
 */
class RobolectricTimeoutEnforcingRunListener
@JvmOverloads
constructor(
    private val testResultsOutputSender: TestResultsOutputSender,
    private val watchdogExecutor: ScheduledExecutorService = Executors.newScheduledThreadPool(1),
) : RunListener() {
  private val activeTimeouts: MutableMap<Description, TestExecution> = ConcurrentHashMap()

  override fun testStarted(description: Description) {
    val startTime = System.currentTimeMillis()

    val timeoutMs = getTimeout()

    // Schedule timeout task
    val timeoutTask =
        watchdogExecutor.schedule(
            { handleTimeout(description, timeoutMs) },
            timeoutMs,
            TimeUnit.MILLISECONDS,
        )

    activeTimeouts[description] = TestExecution(timeoutTask, startTime)
  }

  override fun testFinished(description: Description) {
    cancelExecution(description)
  }

  override fun testFailure(failure: Failure) {
    cancelExecution(failure.description)
  }

  override fun testAssumptionFailure(failure: Failure) {
    cancelExecution(failure.description)
  }

  override fun testIgnored(description: Description) {
    cancelExecution(description)
  }

  private fun cancelExecution(description: Description) {
    val execution = activeTimeouts.remove(description)
    execution?.timeoutTask?.cancel(false)
  }

  private fun getTimeout(): Long {
    // Default timeout is 60 seconds
    val defaultTimeout = 60 * 1000L

    val buckMultiplierProperty = System.getProperty("android.per.test.timeout.multiplier")
    val multiplier = buckMultiplierProperty?.toIntOrNull()?.coerceAtLeast(1) ?: 1

    return defaultTimeout * multiplier
  }

  /** Handles a test timeout by writing a TPX event and forcibly terminating the JVM. */
  private fun handleTimeout(description: Description, timeoutMs: Long) {
    val execution = activeTimeouts[description] ?: return

    val threadDump = ThreadDumpUtils.capture()
    writeTimeoutEvent(description, execution.startTime, timeoutMs, threadDump)

    // This is necessary because Robolectric tests may not respond to thread interruption
    System.exit(1)
  }

  /** Writes a timeout event to the TPX output file. */
  private fun writeTimeoutEvent(
      description: Description,
      startTime: Long,
      timeoutMs: Long,
      threadDump: String,
  ) {
    try {
      val testName = "${description.methodName} (${description.className})"
      val endedTime = System.currentTimeMillis()
      val duration = endedTime - startTime

      val timeoutMessage =
          "Test timed out after ${timeoutMs}ms. " +
              "If your test needs to run longer than ${timeoutMs / 1000} seconds, add the tpx long_running or glacial tag in the labels section of the BUCK target. " +
              "See https://fb.workplace.com/groups/android.testing.fyi/permalink/2679204925789466/ for more details" +
              "\n\n=== Thread Dump ===\n$threadDump"

      // Write finish event with failure
      testResultsOutputSender.sendTestFinish(
          testName,
          TestStatus.FAIL,
          endedTime,
          duration,
          Optional.of(timeoutMessage),
      )
    } catch (e: Exception) {
      System.err.println("Failed to write TPX timeout event: ${e.message}")
      e.printStackTrace()
    }
  }

  /** Shuts down the watchdog executor when testing is complete. */
  fun shutdown() {
    watchdogExecutor.shutdownNow()
  }

  /** Internal class to track test execution state. */
  private data class TestExecution(val timeoutTask: ScheduledFuture<*>, val startTime: Long)
}
