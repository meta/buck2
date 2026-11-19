/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.ksp

import java.lang.management.ManagementFactory
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * Measures the CPU time a KSP2 step uses.
 *
 * Process CPU counts every thread of the JVM, including the JIT compiler and GC threads, which do
 * most of the work in a short-lived compile JVM. It belongs to the step only when nothing else runs
 * in the JVM meanwhile; [Reading.overlappingSteps] counts the other KSP2 steps that ran at the same
 * time. Thread CPU counts only the thread that runs the step.
 */
object Ksp2CpuMeter {

  private val stepsInFlight = AtomicInteger()
  private val stepsStarted = AtomicLong()

  data class Reading(
      val processCpuMs: Long?,
      val threadCpuMs: Long?,
      val overlappingSteps: Long,
  ) {
    fun toExtrasValue(): String =
        "process_cpu_ms=${processCpuMs ?: -1},thread_cpu_ms=${threadCpuMs ?: -1}," +
            "overlapping_ksp2_steps=$overlappingSteps"
  }

  fun <T> measure(block: () -> T): Pair<T, Reading> {
    val inFlightAtStart = stepsInFlight.incrementAndGet()
    val startedAtStart = stepsStarted.incrementAndGet()
    val processStart = processCpuNanos()
    val threadStart = threadCpuNanos()
    try {
      val result = block()
      val reading = Reading(
          processCpuMs = elapsedMs(processStart, processCpuNanos()),
          threadCpuMs = elapsedMs(threadStart, threadCpuNanos()),
          overlappingSteps = (inFlightAtStart - 1) + (stepsStarted.get() - startedAtStart),
      )
      return result to reading
    } finally {
      stepsInFlight.decrementAndGet()
    }
  }

  private fun elapsedMs(startNanos: Long?, endNanos: Long?): Long? =
      if (startNanos == null || endNanos == null) null else (endNanos - startNanos) / 1_000_000

  private fun processCpuNanos(): Long? =
      (ManagementFactory.getOperatingSystemMXBean() as? com.sun.management.OperatingSystemMXBean)
          ?.processCpuTime
          ?.takeIf { it >= 0 }

  private fun threadCpuNanos(): Long? {
    val threads = ManagementFactory.getThreadMXBean()
    if (!threads.isCurrentThreadCpuTimeSupported) {
      return null
    }
    return threads.currentThreadCpuTime.takeIf { it >= 0 }
  }
}
