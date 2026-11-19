/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.analytics.logger

import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersionForLogs
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDAnalytics
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
import com.facebook.buck.jvm.kotlin.cd.analytics.ModeParam
import com.facebook.buck.jvm.kotlin.cd.analytics.logger.model.KotlinCDLogEntry
import com.google.common.util.concurrent.ThreadFactoryBuilder
import java.time.Clock
import java.time.Duration
import java.time.Instant
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.Executor
import java.util.concurrent.RejectedExecutionHandler
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit

class KotlinCDLoggerAnalytics
@JvmOverloads
constructor(
    private val kotlinCDLogger: KotlinCDLogger,
    private val buildUuid: String?,
    private val target: String,
    private val subtarget: String,
    private val executionPlatform: String,
    private val numJavaFiles: Long,
    private val numKotlinFiles: Long,
    private val incremental: Boolean,
    private val clock: Clock = Clock.systemDefaultZone(),
    private val executor: Executor = defaultLoggingExecutor,
) : KotlinCDAnalytics() {

  override fun log(context: KotlinCDLoggingContext) {
    if (buildUuid == null) {
      LOG.debug(
          "No operation performed. This is expected when running an action downloaded from RE.",
      )
      return
    }

    // Build the log entry on the calling thread so its timestamps reflect when the
    // action finished (and so an invalid language version fails fast here), then hand
    // the blocking scribe write off the critical path.
    val logEntry = createKotlinCDLogEntry(context)
    executor.execute { writeLogEntry(logEntry) }
  }

  private fun writeLogEntry(logEntry: KotlinCDLogEntry) {
    val start = Instant.now(clock)
    val success: Boolean = kotlinCDLogger.log(logEntry)
    val end = Instant.now(clock)

    val duration = Duration.between(start, end)
    if (success) {
      LOG.info(
          "Successfully wrote KotlinCD logs to scribe. Total time: " +
              duration.toMillis() +
              " milliseconds",
      )
    } else {
      LOG.warn(
          (("Failed to write KotlinCD logs to scribe. Total time: " +
              duration.toMillis() +
              " milliseconds")),
      )
    }
  }

  @OptIn(LanguageVersionForLogs::class)
  private fun createKotlinCDLogEntry(context: KotlinCDLoggingContext): KotlinCDLogEntry {
    val addedAndModifiedFiles: Set<String>? =
        (context.mode as? ModeParam.Incremental)
            ?.addedAndModifiedFiles
            ?.map { it.toString() }
            ?.toSet()
    val removedFiles: Set<String>? =
        (context.mode as? ModeParam.Incremental)?.removedFiles?.map { it.toString() }?.toSet()

    return KotlinCDLogEntry(
        time = Instant.now(clock).epochSecond,
        eventTime = Instant.now(clock).epochSecond.toDouble(),
        target = target,
        subtarget = subtarget,
        buildUuid = requireNotNull(buildUuid) { "buildUuid is required" },
        executionPlatform = executionPlatform,
        numKotlinFiles = numKotlinFiles,
        numJavaFiles = numJavaFiles,
        incremental = incremental,
        mode = context.mode?.value,
        classpathChanges = context.classpathChangesParam?.value,
        step = context.step.value,
        languageVersion = context.languageVersion.valueForLogs,
        extras = buildJson(context.extras),
        addedAndModifiedFiles = addedAndModifiedFiles,
        removedFiles = removedFiles,
        numKotlinTokens = context.numKotlinTokens.takeIf { it > 0 },
        numJavaTokens = context.numJavaTokens.takeIf { it > 0 },
        durationMs = context.durationMs,
    )
  }

  private fun buildJson(extras: Map<String, List<String>>): String? {
    if (extras.isEmpty()) {
      return null
    }

    return "{${extras.map { (key, list) -> "\"${key}\": [${list.joinToString(transform = { "\"$it\"" })}]" }.joinToString()}}"
  }

  companion object {
    private val LOG: Logger = Logger.get(KotlinCDLoggerAnalytics::class.java)

    private const val LOGGER_THREAD_POOL_SIZE = 4
    private const val MAX_QUEUED_LOG_ENTRIES = 1024

    /**
     * Scribe writes go through the `logger_cat` subprocess, which can block for several seconds
     * under host load. That call previously ran on the compile action's critical path, so a slow
     * `logger_cat` stalled action completion and starved worker threads. The write is offloaded
     * here to a small, bounded, daemon-thread pool: the fixed thread count and capped queue keep a
     * degraded `logger_cat` from growing the long-lived worker's heap, and saturating entries are
     * dropped rather than queued without bound, since these analytics are best-effort.
     */
    private val defaultLoggingExecutor: Executor = ThreadPoolExecutor(
        LOGGER_THREAD_POOL_SIZE,
        LOGGER_THREAD_POOL_SIZE,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(MAX_QUEUED_LOG_ENTRIES),
        ThreadFactoryBuilder().setDaemon(true).setNameFormat("kotlincd-scribe-logger-%d").build(),
        RejectedExecutionHandler { _, _ ->
          LOG.debug("Dropped KotlinCD scribe log entry; logging queue is full")
        },
    )
  }
}
