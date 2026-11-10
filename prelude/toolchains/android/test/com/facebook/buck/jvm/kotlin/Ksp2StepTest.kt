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

import com.facebook.buck.jvm.kotlin.ksp.Ksp2NoOpDetector
import com.facebook.buck.jvm.kotlin.ksp.Ksp2Step
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class Ksp2StepTest {

  @Test
  fun `records counts for successful runs`() {
    assertTrue(Ksp2Step.shouldRecordProcessorCounts(true, true))
  }

  @Test
  fun `skips counts on failure`() {
    assertFalse(Ksp2Step.shouldRecordProcessorCounts(false, true))
  }

  @Test
  fun `skips counts when empty`() {
    assertFalse(Ksp2Step.shouldRecordProcessorCounts(true, false))
  }

  @Test
  fun `processor times format as create, process, finish and rounds per provider`() {
    assertEquals(
        "a.P=1;20;3;2,b.Q=0;0;0;1",
        Ksp2Step.formatProcessorTimes(
            linkedMapOf(
                "a.P" to Ksp2NoOpDetector.ProcessorTime(1, 20, 3, 2),
                "b.Q" to Ksp2NoOpDetector.ProcessorTime(0, 0, 0, 1),
            ),
        ),
    )
  }

  @Test
  fun `phases format in order and are omitted when not measured`() {
    assertEquals(
        "setup=100,processors=67,other=10",
        Ksp2Step.formatPhases(linkedMapOf("setup" to 100L, "processors" to 67L, "other" to 10L)),
    )
    assertNull(Ksp2Step.formatPhases(emptyMap()))
  }

  @Test
  fun `without friend paths the classpath is unchanged`() {
    assertEquals(
        listOf(LIB, FRIEND) to emptyList<File>(),
        Ksp2Step.librariesAndFriends(listOf(LIB, FRIEND), emptyList()),
    )
  }

  @Test
  fun `friend jars move from the libraries to the friends`() {
    assertEquals(
        listOf(OTHER, LIB) to listOf(FRIEND),
        Ksp2Step.librariesAndFriends(listOf(OTHER, FRIEND, LIB), listOf(FRIEND)),
    )
  }

  @Test
  fun `a friend jar missing from the classpath is still a friend`() {
    assertEquals(
        listOf(LIB) to listOf(FRIEND),
        Ksp2Step.librariesAndFriends(listOf(LIB), listOf(FRIEND)),
    )
  }

  private companion object {
    val LIB = File("/out/lib/class-abi.jar")
    val OTHER = File("/out/other/class-abi.jar")
    val FRIEND = File("/out/friend/class-abi.jar")
  }
}
