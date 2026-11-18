/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.util.perf;

import com.facebook.infer.annotation.Nullsafe;
import com.google.common.collect.ImmutableMap;
import java.lang.management.GarbageCollectorMXBean;
import java.lang.management.ManagementFactory;
import java.lang.management.MemoryPoolMXBean;
import java.lang.management.MemoryType;
import java.util.Map;
import java.util.Objects;

/** Periodically probes for process-wide perf-related metrics. */
@Nullsafe(Nullsafe.Mode.LOCAL)
public class PerfStatsTracking {

  /**
   * Creates {@link MemoryPerfStatsEvent} using the current env state (used/free memory values, GC
   * time spend so far and so on )
   */
  public static MemoryPerfStatsEvent getMemoryPerfStatsEvent() {
    long freeMemoryBytes = Runtime.getRuntime().freeMemory();
    long totalMemoryBytes = Runtime.getRuntime().totalMemory();
    long maxMemoryBytes = Runtime.getRuntime().maxMemory();

    long totalGcTimeMs = 0;
    for (GarbageCollectorMXBean gcMxBean : ManagementFactory.getGarbageCollectorMXBeans()) {
      long collectionTimeMs = gcMxBean.getCollectionTime();
      if (collectionTimeMs == -1) {
        // Gc collection time is not supported on this JVM.
        totalGcTimeMs = -1;
        break;
      }
      totalGcTimeMs += collectionTimeMs;
    }

    ImmutableMap.Builder<String, Long> currentMemoryBytesUsageByPool = ImmutableMap.builder();
    for (MemoryPoolMXBean memoryPoolBean : ManagementFactory.getMemoryPoolMXBeans()) {
      String name = memoryPoolBean.getName();
      MemoryType type = memoryPoolBean.getType();
      long currentlyUsedBytes = Objects.requireNonNull(memoryPoolBean.getUsage()).getUsed();
      currentMemoryBytesUsageByPool.put(name + "(" + type + ")", currentlyUsedBytes);
    }

    return new MemoryPerfStatsEvent(
        freeMemoryBytes,
        totalMemoryBytes,
        maxMemoryBytes,
        totalGcTimeMs,
        currentMemoryBytesUsageByPool.build());
  }

  /** Performance event that tracks current memory usage of Buck */
  public static class MemoryPerfStatsEvent {

    private final long freeMemoryBytes;
    private final long totalMemoryBytes;
    private final long maxMemoryBytes;
    private final long timeSpentInGcMs;
    private final Map<String, Long> currentMemoryBytesUsageByPool;

    /**
     * Construct a new memory performance tracking object
     *
     * @param freeMemoryBytes Memory in bytes available for JVM to use for new allocations
     * @param totalMemoryBytes Memory in bytes that JVM allocated at the moment, both used and
     *     unused
     * @param maxMemoryBytes Maximum amount of memory in bytes that JVM can allocate (-Xmx
     *     parameter)
     * @param timeSpentInGcMs Total amount of milliseconds spent doing garbage collection till now
     * @param currentMemoryBytesUsageByPool A map of JVM memory pool name to the amount of memory
     *     used by that pool
     */
    public MemoryPerfStatsEvent(
        long freeMemoryBytes,
        long totalMemoryBytes,
        long maxMemoryBytes,
        long timeSpentInGcMs,
        Map<String, Long> currentMemoryBytesUsageByPool) {
      this.freeMemoryBytes = freeMemoryBytes;
      this.totalMemoryBytes = totalMemoryBytes;
      this.maxMemoryBytes = maxMemoryBytes;
      this.timeSpentInGcMs = timeSpentInGcMs;
      this.currentMemoryBytesUsageByPool = currentMemoryBytesUsageByPool;
    }

    /**
     * @return Memory in bytes available for JVM to use for new allocations
     */
    public long getFreeMemoryBytes() {
      return freeMemoryBytes;
    }

    /**
     * @return Memory in bytes that JVM allocated at the moment, both used and unused
     */
    public long getTotalMemoryBytes() {
      return totalMemoryBytes;
    }

    /**
     * @return Maximum amount of memory in bytes that JVM can allocate (-Xmx parameter)
     */
    public long getMaxMemoryBytes() {
      return maxMemoryBytes;
    }

    /**
     * @return Total amount of milliseconds spent doing garbage collection till now
     */
    public long getTimeSpentInGcMs() {
      return timeSpentInGcMs;
    }

    /**
     * @return A map of JVM memory pool name to the amount of memory used by that pool
     */
    public Map<String, Long> getCurrentMemoryBytesUsageByPool() {
      return currentMemoryBytesUsageByPool;
    }
  }
}
