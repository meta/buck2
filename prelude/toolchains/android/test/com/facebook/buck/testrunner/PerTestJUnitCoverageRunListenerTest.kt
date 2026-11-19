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

import java.io.File
import java.util.ServiceConfigurationError
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.Description

/**
 * Tests for [PerTestJUnitCoverageRunListener]-specific behavior — the per-run discriminator that
 * keeps `_setup`/`_teardown` filenames unique across the multiple `jUnitCore.run()` invocations
 * that `JUnitRunner` issues per test class, and the [ReflectiveCoverageAgent] default constructor
 * behavior when the JaCoCo runtime agent is not on the classpath. The shared lifecycle is tested in
 * [BasePerTestCoverageRunListenerTest].
 */
class PerTestJUnitCoverageRunListenerTest {

  @get:Rule val tempDir = TemporaryFolder()

  @Test
  fun `setup and teardown include run count suffix`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = PerTestJUnitCoverageRunListener(outputDir, agent)

    val desc = Description.createSuiteDescription("classes")
    listener.testRunStarted(desc)
    listener.testRunFinished(null)
    listener.close()

    assertTrue(
        "Setup includes run discriminator",
        File(outputDir, "_setup_classes_1.exec").exists(),
    )
    assertTrue(
        "Teardown includes run discriminator",
        File(outputDir, "_teardown_classes_1.exec").exists(),
    )
  }

  @Test
  fun `run count increments across multiple runs`() {
    // Simulates the JUnitRunner pattern: jUnitCore.run() invoked once per test class, with the
    // same listener instance reused. Without the run counter, the second run's setup/teardown
    // dumps would overwrite the first.
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = PerTestJUnitCoverageRunListener(outputDir, agent)

    val desc = Description.createSuiteDescription("classes")
    listener.testRunStarted(desc)
    listener.testRunFinished(null)
    listener.testRunStarted(desc)
    listener.testRunFinished(null)
    listener.close()

    assertTrue(File(outputDir, "_setup_classes_1.exec").exists())
    assertTrue(File(outputDir, "_teardown_classes_1.exec").exists())
    assertTrue(File(outputDir, "_setup_classes_2.exec").exists())
    assertTrue(File(outputDir, "_teardown_classes_2.exec").exists())
  }

  @Test
  fun `default constructor throws without JaCoCo agent`() {
    val outputDir = tempDir.newFolder("coverage")
    try {
      PerTestJUnitCoverageRunListener(outputDir)
      fail("Expected ClassNotFoundException")
    } catch (e: ClassNotFoundException) {
      // Expected: JaCoCo agent is not on the test classpath
    }
  }

  @Test
  fun `coverage extensions follow the per-test listener lifecycle`() {
    val outputDir = tempDir.newFolder("coverage")
    val events = ArrayList<String>()
    val extension =
        object : PerTestCoverageExtension {
          override fun initialize(outputDir: File) {
            events.add("initialize:${outputDir.name}")
          }

          override fun testStarted(testName: String) {
            events.add("started:$testName")
          }

          override fun testFinished(testName: String) {
            events.add("finished:$testName")
          }

          override fun close() {
            events.add("close")
          }
        }
    val listener = PerTestJUnitCoverageRunListener(
        outputDir,
        FakeCoverageAgent(byteArrayOf(1)),
        PerTestCoverageExtensionManager(listOf(extension)),
    )
    val test = Description.createTestDescription("Example", "test")
    listener.testStarted(test)
    listener.testFinished(test)
    listener.close()

    assertEquals(
        listOf(
            "initialize:coverage",
            "started:test (Example)",
            "finished:test (Example)",
            "close",
        ),
        events,
    )
  }

  @Test
  fun `coverage extension failures do not fail primary coverage`() {
    val outputDir = tempDir.newFolder("coverage")
    val extension =
        object : PerTestCoverageExtension {
          override fun initialize(outputDir: File) {
            throw IllegalStateException("optional collector unavailable")
          }

          override fun testStarted(testName: String) {
            throw IllegalStateException("optional collector unavailable")
          }
        }
    val listener = PerTestJUnitCoverageRunListener(
        outputDir,
        FakeCoverageAgent(byteArrayOf(1)),
        PerTestCoverageExtensionManager(listOf(extension)),
    )
    val test = Description.createTestDescription("Example", "test")

    listener.testStarted(test)
    listener.testFinished(test)
    listener.close()

    assertTrue("Extension failure does not set coverageError", listener.coverageError == null)
    assertTrue(File(outputDir, "test_(Example).exec").exists())
  }

  @Test
  fun `provider discovery continues after a provider fails`() {
    val events = ArrayList<String>()
    val extension =
        object : PerTestCoverageExtension {
          override fun initialize(outputDir: File) {
            events.add("initialized")
          }
        }
    var providerIndex = 0
    val providers =
        object : Iterator<PerTestCoverageExtension> {
          override fun hasNext(): Boolean = providerIndex < 2

          override fun next(): PerTestCoverageExtension {
            providerIndex += 1
            if (providerIndex == 1) throw ServiceConfigurationError("broken provider")
            return extension
          }
        }

    val manager = PerTestCoverageExtensionManager.load(providers)
    manager.initialize(tempDir.newFolder("coverage"))

    assertEquals(listOf("initialized"), events)
  }

  /** Fake [BasePerTestCoverageRunListener.CoverageAgent] returning fixed data. */
  private class FakeCoverageAgent(private val execData: ByteArray?) :
      BasePerTestCoverageRunListener.CoverageAgent {
    override fun reset() {}

    override fun dumpAndReset(): ByteArray? = execData
  }
}
