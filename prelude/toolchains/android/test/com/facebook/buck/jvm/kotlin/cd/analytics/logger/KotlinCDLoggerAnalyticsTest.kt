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

import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.jvm.cd.command.kotlin.LanguageVersion
import com.facebook.buck.jvm.kotlin.cd.analytics.ClasspathChangesParam
import com.facebook.buck.jvm.kotlin.cd.analytics.KotlinCDLoggingContext
import com.facebook.buck.jvm.kotlin.cd.analytics.ModeParam
import com.facebook.buck.jvm.kotlin.cd.analytics.StepParam
import com.facebook.buck.jvm.kotlin.cd.analytics.logger.model.KotlinCDLogEntry
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import java.util.concurrent.Executor
import org.junit.Test
import org.mockito.kotlin.any
import org.mockito.kotlin.mock
import org.mockito.kotlin.never
import org.mockito.kotlin.times
import org.mockito.kotlin.verify

internal class KotlinCDLoggerAnalyticsTest {

  private val kotlinCDLogger: KotlinCDLogger = mock()

  private val clock = Clock.fixed(Instant.parse("2018-08-22T10:00:00Z"), ZoneOffset.UTC)

  private val directExecutor = Executor { it.run() }

  @Test
  fun `when buildUuid is null, data are not logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics(buildUuid = null)
    kotlinCDAnalytics.log(createKotlinCDLoggingContext())

    verify(kotlinCDLogger, never()).log(any())
  }

  @Test
  fun `when buildUuid is not null, data are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    kotlinCDAnalytics.log(createKotlinCDLoggingContext())

    verify(kotlinCDLogger, times(1)).log(any())
  }

  @Test
  fun `when language version has major-minor format, it is logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry(languageVersion = "2.0")

    kotlinCDAnalytics.log(createKotlinCDLoggingContext(languageVersion = "2.0"))

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when language version has major-minor-path format, it is logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry(languageVersion = "2.0.0")

    kotlinCDAnalytics.log(createKotlinCDLoggingContext(languageVersion = "2.0.0"))

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test(expected = IllegalArgumentException::class)
  fun `when language version is not valid, exception is thrown`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()

    kotlinCDAnalytics.log(createKotlinCDLoggingContext(languageVersion = "languageVersion"))
  }

  @Test
  fun `when extras contains a single key-value pair, correct json data are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry(extras = """{"testKey": ["testValue"]}""")

    kotlinCDAnalytics.log(
        createKotlinCDLoggingContext(extras = mapOf("testKey" to listOf("testValue"))),
    )

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when extras contains a single key-list pair, correct json data are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry =
        createExpectedKotlinCDLogEntry(extras = """{"testKey": ["testValue1", "testValue2"]}""")

    kotlinCDAnalytics.log(
        createKotlinCDLoggingContext(
            extras = mapOf("testKey" to listOf("testValue1", "testValue2")),
        ),
    )

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when extras contains a list of key-list pairs, correct json data are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry(
        extras =
            """{"testKey1": ["testValue1", "testValue2"], "testKey2": ["testValue3", "testValue4"]}""",
    )

    kotlinCDAnalytics.log(
        createKotlinCDLoggingContext(
            extras =
                mapOf(
                    "testKey1" to listOf("testValue1", "testValue2"),
                    "testKey2" to listOf("testValue3", "testValue4"),
                ),
        ),
    )

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when there are modified files, they are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()

    val expectedEntry = createExpectedKotlinCDLogEntry(modifiedFiles = setOf("/A", "/B"))

    kotlinCDAnalytics.log(
        createKotlinCDLoggingContext(
            kotlincMode =
                ModeParam.Incremental(
                    ClasspathChangesParam.NO_CHANGES,
                    setOf(AbsPath.get("/B"), AbsPath.get("/A")),
                    emptySet(),
                ),
        ),
    )

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when there are removed files, they are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()

    val expectedEntry = createExpectedKotlinCDLogEntry(removedFiles = setOf("/A", "/B"))

    kotlinCDAnalytics.log(
        createKotlinCDLoggingContext(
            kotlincMode =
                ModeParam.Incremental(
                    ClasspathChangesParam.NO_CHANGES,
                    emptySet(),
                    setOf(AbsPath.get("/B"), AbsPath.get("/A")),
                ),
        ),
    )

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when token counts are set, they are logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry =
        createExpectedKotlinCDLogEntry(numKotlinTokens = 1000L, numJavaTokens = 500L)

    kotlinCDAnalytics.log(
        createKotlinCDLoggingContext(numKotlinTokens = 1000L, numJavaTokens = 500L),
    )

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when token counts are zero, they are not logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry()

    kotlinCDAnalytics.log(createKotlinCDLoggingContext(numKotlinTokens = 0L, numJavaTokens = 0L))

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when a step duration is set, it is logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry(durationMs = 1234L)

    kotlinCDAnalytics.log(createKotlinCDLoggingContext(durationMs = 1234L))

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  @Test
  fun `when no step duration is set, none is logged`() {
    val kotlinCDAnalytics = createFakeKotlinCDAnalytics()
    val expectedEntry = createExpectedKotlinCDLogEntry()

    kotlinCDAnalytics.log(createKotlinCDLoggingContext())

    verify(kotlinCDLogger, times(1)).log(expectedEntry)
  }

  private fun createKotlinCDLoggingContext(
      step: StepParam = StepParam.KOTLINC,
      languageVersion: String = DEFAULT_LANGUAGE_VERSION,
      kotlincMode: ModeParam? =
          ModeParam.Incremental(ClasspathChangesParam.NO_CHANGES, emptySet(), emptySet()),
      extras: Map<String, List<String>> = mapOf(),
      numKotlinTokens: Long = 0L,
      numJavaTokens: Long = 0L,
      durationMs: Long? = null,
  ): KotlinCDLoggingContext {
    val context = KotlinCDLoggingContext(step, LanguageVersion(languageVersion), kotlincMode)
    extras.forEach { (key, extras) -> extras.forEach { item -> context.addExtras(key, item) } }
    context.numKotlinTokens = numKotlinTokens
    context.numJavaTokens = numJavaTokens
    context.durationMs = durationMs
    return context
  }

  private fun createFakeKotlinCDAnalytics(
      buildUuid: String? = DEFAULT_BUILDUUID,
  ) = KotlinCDLoggerAnalytics(
      kotlinCDLogger = kotlinCDLogger,
      buildUuid = buildUuid,
      target = TARGET,
      subtarget = SUBTARGET,
      executionPlatform = EXECTION_PLATFORM,
      numJavaFiles = NUM_JAVA_FILES,
      numKotlinFiles = NUM_KOTLIN_FILES,
      incremental = INCREMENTAL,
      clock = clock,
      executor = directExecutor,
  )

  private fun createExpectedKotlinCDLogEntry(
      step: StepParam = StepParam.KOTLINC,
      languageVersion: String? = DEFAULT_LANGUAGE_VERSION,
      kotlincMode: ModeParam? =
          ModeParam.Incremental(ClasspathChangesParam.NO_CHANGES, emptySet(), emptySet()),
      extras: String? = null,
      modifiedFiles: Set<String> = emptySet(),
      removedFiles: Set<String> = emptySet(),
      numKotlinTokens: Long? = null,
      numJavaTokens: Long? = null,
      durationMs: Long? = null,
  ) = KotlinCDLogEntry(
      time = Instant.now(clock).epochSecond,
      eventTime = Instant.now(clock).epochSecond.toDouble(),
      target = TARGET,
      subtarget = SUBTARGET,
      buildUuid = BUILD_UUID,
      executionPlatform = EXECTION_PLATFORM,
      numKotlinFiles = NUM_KOTLIN_FILES,
      numJavaFiles = NUM_JAVA_FILES,
      incremental = INCREMENTAL,
      mode = kotlincMode?.value,
      classpathChanges = (kotlincMode as? ModeParam.Incremental)?.classpathChangesParam?.value,
      step = step.value,
      languageVersion = languageVersion,
      extras = extras,
      addedAndModifiedFiles = modifiedFiles,
      removedFiles = removedFiles,
      numKotlinTokens = numKotlinTokens,
      numJavaTokens = numJavaTokens,
      durationMs = durationMs,
  )

  companion object TestParams {
    private const val TARGET = "target"
    private const val SUBTARGET = "subtarget"
    private const val BUILD_UUID = "buildUuid"
    private const val EXECTION_PLATFORM = "executionPlatform"
    private const val NUM_KOTLIN_FILES = 5L
    private const val NUM_JAVA_FILES = 10L
    private const val INCREMENTAL = true
    private const val DEFAULT_BUILDUUID = "buildUuid"
    private const val DEFAULT_LANGUAGE_VERSION = "2.0.0"
  }
}
