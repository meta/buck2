/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin

import com.facebook.buck.jvm.kotlin.ksp.Ksp2CpuMeter
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class Ksp2CpuMeterTest {

  @Test
  fun `a busy step reads thread and process CPU`() {
    val (result, reading) = Ksp2CpuMeter.measure { spin(millis = 300) }
    assertEquals(SPUN, result)
    val threadMs = checkNotNull(reading.threadCpuMs)
    val processMs = checkNotNull(reading.processCpuMs)
    assertTrue("thread CPU $threadMs ms", threadMs >= 30)
    assertTrue("process CPU $processMs ms < thread CPU $threadMs ms", processMs >= threadMs - 20)
    assertEquals(0L, reading.overlappingSteps)
  }

  @Test
  fun `a step that starts during another counts as overlapping`() {
    val outerStarted = CountDownLatch(1)
    val innerDone = CountDownLatch(1)
    var outer: Ksp2CpuMeter.Reading? = null
    val outerThread = thread {
      outer =
          Ksp2CpuMeter.measure {
                outerStarted.countDown()
                innerDone.await(10, TimeUnit.SECONDS)
              }
              .second
    }
    assertTrue(outerStarted.await(10, TimeUnit.SECONDS))
    val inner = Ksp2CpuMeter.measure {}.second
    innerDone.countDown()
    outerThread.join()
    assertEquals(1L, inner.overlappingSteps)
    assertEquals(1L, outer!!.overlappingSteps)
  }

  @Test
  fun `the extras value names every field`() {
    assertEquals(
        "process_cpu_ms=1200,thread_cpu_ms=800,overlapping_ksp2_steps=0",
        Ksp2CpuMeter.Reading(1200, 800, 0).toExtrasValue(),
    )
    assertEquals(
        "process_cpu_ms=-1,thread_cpu_ms=-1,overlapping_ksp2_steps=2",
        Ksp2CpuMeter.Reading(null, null, 2).toExtrasValue(),
    )
  }

  private fun spin(millis: Long): String {
    val end = System.nanoTime() + millis * 1_000_000
    var x = 0L
    while (System.nanoTime() < end) {
      x += x xor System.nanoTime()
    }
    return if (x == Long.MIN_VALUE) "never" else SPUN
  }

  private companion object {
    const val SPUN = "spun"
  }
}
