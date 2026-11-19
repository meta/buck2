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

import com.facebook.buck.testresultsoutput.TestResultsOutputEvent.RunFailureStatus
import com.facebook.buck.testresultsoutput.TestResultsOutputSender
import java.util.concurrent.ScheduledExecutorService
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Writes timeout diagnostics (thread dumps, run failure events) to the TPX output file ~5 seconds
 * before the action timeout, ensuring they are captured before RE's SIGKILL.
 *
 * Reads `TPX_TIMEOUT_SEC` from the environment and schedules a timer for `(TPX_TIMEOUT_SEC - 5)`
 * seconds. When the timer fires, it captures a thread dump and writes a TIMEOUT run failure event.
 * The [markCompleted] method cancels the timer on normal exit.
 */
class TpxTimeoutBufferManager
private constructor(
    private val sender: TestResultsOutputSender,
) {

  private val completed = AtomicBoolean(false)
  private var timerFuture: ScheduledFuture<*>? = null
  private var scheduler: ScheduledExecutorService? = null

  /** Marks the run as completed so the proactive timer does not fire. */
  fun markCompleted() {
    completed.set(true)
    timerFuture?.cancel(false)
    scheduler?.shutdown()
  }

  private fun startProactiveTimer(tpxTimeoutSec: Long, scheduler: ScheduledExecutorService) {
    this.scheduler = scheduler
    val effectiveTimeout = tpxTimeoutSec - PROACTIVE_TIMEOUT_BUFFER_SEC
    if (effectiveTimeout <= 0) {
      return
    }
    timerFuture =
        scheduler.schedule(
            {
              if (completed.compareAndSet(false, true)) {
                handleTimeout()
              }
            },
            effectiveTimeout,
            TimeUnit.SECONDS,
        )
  }

  private fun handleTimeout() {
    val threadDump = ThreadDumpUtils.capture()
    val now = System.currentTimeMillis()

    // Report run failure with TIMEOUT status.
    try {
      sender.sendRunFailure(RunFailureStatus.TIMEOUT, now, "Test target timed out", null)
    } catch (e: Exception) {
      System.err.println("Failed to report run failure: ${e.message}")
    }

    // Write thread dump directly to fd 2, bypassing StandardOutputRecorder which may have
    // intercepted System.err. TPX includes process stderr in the timeout message on the main
    // test, so this is how the thread dump reaches the diagnostics page.
    java.io.FileOutputStream(java.io.FileDescriptor.err).use { fos ->
      val realStderr = java.io.PrintStream(fos)
      realStderr.println("\n=== Thread Dump at Timeout ===")
      realStderr.println(threadDump)
      realStderr.flush()
    }

    scheduler?.shutdown()
  }

  companion object {
    private const val PROACTIVE_TIMEOUT_BUFFER_SEC = 5L
    private const val TPX_TIMEOUT_ENV_VAR = "TPX_TIMEOUT_SEC"

    /**
     * Creates a manager and starts a proactive timeout timer if `TPX_TIMEOUT_SEC` is set in the
     * environment and the effective timeout is positive.
     */
    @JvmStatic
    fun create(
        sender: TestResultsOutputSender,
        scheduler: ScheduledExecutorService,
    ): TpxTimeoutBufferManager {
      val manager = TpxTimeoutBufferManager(sender)
      val tpxTimeoutSec = System.getenv(TPX_TIMEOUT_ENV_VAR)?.toLongOrNull()
      if (tpxTimeoutSec != null) {
        manager.startProactiveTimer(tpxTimeoutSec, scheduler)
      }
      return manager
    }

    /**
     * Creates a manager with an explicit timeout value.
     *
     * @param tpxTimeoutSec the total action timeout in seconds.
     * @param scheduler the executor used to schedule the proactive timer.
     */
    @JvmStatic
    fun create(
        sender: TestResultsOutputSender,
        tpxTimeoutSec: Long,
        scheduler: ScheduledExecutorService,
    ): TpxTimeoutBufferManager {
      val manager = TpxTimeoutBufferManager(sender)
      manager.startProactiveTimer(tpxTimeoutSec, scheduler)
      return manager
    }
  }
}
