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

import com.facebook.buck.testresultsoutput.TestResultsOutputSender
import java.io.BufferedReader
import java.io.File
import java.io.FileOutputStream
import java.io.FileReader
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.ScheduledThreadPoolExecutor
import java.util.concurrent.TimeUnit
import org.junit.Assert
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

/** Tests [TpxTimeoutBufferManager] */
class TpxTimeoutBufferManagerTest {
  lateinit var tempFile: File

  @get:Rule val folder = TemporaryFolder()

  @Before
  fun setUp() {
    tempFile = folder.newFile("test_results.json")
  }

  @Test
  fun testProactiveTimerSkippedWhenTimeoutTooSmall() {
    val scheduler = ManualScheduler()
    FileOutputStream(tempFile).use { fos ->
      val sender = TestResultsOutputSender(fos)
      // Edge case but if people want to essentially force a timeout we should let them
      val manager =
          TpxTimeoutBufferManager.create(sender, tpxTimeoutSec = 3L, scheduler = scheduler)

      manager.markCompleted()
    }

    Assert.assertFalse(
        "Expected no scheduled tasks when timeout is too small for proactive timer",
        scheduler.hasScheduledTasks(),
    )
    BufferedReader(FileReader(tempFile)).use { reader ->
      Assert.assertNull(
          "Expected no events when timeout is too small for proactive timer",
          reader.readLine(),
      )
    }
  }

  @Test
  fun testMarkCompletedCancelsProactiveTimer() {
    val scheduler = ManualScheduler()
    FileOutputStream(tempFile).use { fos ->
      val sender = TestResultsOutputSender(fos)
      val manager =
          TpxTimeoutBufferManager.create(sender, tpxTimeoutSec = 7L, scheduler = scheduler)

      // Mark completed immediately, before the timer fires
      manager.markCompleted()

      scheduler.triggerAll()
    }

    BufferedReader(FileReader(tempFile)).use { reader ->
      Assert.assertNull(
          "Expected no events when markCompleted() cancels proactive timer",
          reader.readLine(),
      )
    }
  }

  @Test
  fun testProactiveTimerFiresAndWritesOutput() {
    val scheduler = ManualScheduler()
    FileOutputStream(tempFile).use { fos ->
      val sender = TestResultsOutputSender(fos)
      // tpxTimeoutSec=6 means effective timeout = 6-5 = 1 second
      TpxTimeoutBufferManager.create(sender, tpxTimeoutSec = 6L, scheduler = scheduler)

      scheduler.triggerAll()
    }

    val output = tempFile.readText()
    Assert.assertTrue(
        "Expected run_failure event in output",
        output.contains("run_failure"),
    )
    Assert.assertTrue(
        "Expected TIMEOUT status (status=1) in output",
        output.contains("\"status\":1"),
    )
  }

  /**
   * A [ScheduledThreadPoolExecutor] that captures scheduled tasks for manual, deterministic
   * execution in tests instead of running them after a real delay.
   */
  private class ManualScheduler : ScheduledThreadPoolExecutor(1) {
    private val scheduledTasks = mutableListOf<Runnable>()

    override fun schedule(command: Runnable, delay: Long, unit: TimeUnit): ScheduledFuture<*> {
      scheduledTasks.add(command)
      // Return a no-op future
      return super.schedule({}, 0, TimeUnit.NANOSECONDS)
    }

    fun triggerAll() {
      scheduledTasks.toList().forEach { it.run() }
      scheduledTasks.clear()
    }

    fun hasScheduledTasks(): Boolean = scheduledTasks.isNotEmpty()
  }
}
