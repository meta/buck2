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

import com.fasterxml.jackson.core.JsonFactory
import java.io.File
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.Description

/**
 * Tests the lifecycle contract defined by [BasePerTestCoverageRunListener] via a minimal test-only
 * subclass that supplies only a [BasePerTestCoverageRunListener.CoverageAgent]. Subclass-specific
 * behavior (JVM `runCount` discriminator, Android sentinel file) is tested in the respective
 * subclass test files.
 */
class BasePerTestCoverageRunListenerTest {

  @get:Rule val tempDir = TemporaryFolder()

  @Test
  fun `writes exec file per test`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1, 2, 3))
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testBar")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    val execFile = File(outputDir, "testBar_(com.example.FooTest).exec")
    assertTrue("Exec file should be created", execFile.exists())
    assertArrayEquals(byteArrayOf(1, 2, 3), execFile.readBytes())
  }

  @Test
  fun `writes manifest JSONL`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testBar")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    val manifest = File(outputDir, "manifest.jsonl")
    assertTrue("Manifest should exist", manifest.exists())

    val line = manifest.readLines().single()
    JsonFactory().createParser(line).use { parser ->
      parser.nextToken() // START_OBJECT
      parser.nextToken() // FIELD_NAME
      assertEquals("test_name", parser.currentName())
      parser.nextToken()
      assertEquals("testBar (com.example.FooTest)", parser.text)
      parser.nextToken() // FIELD_NAME
      assertEquals("exec_file", parser.currentName())
      parser.nextToken()
      assertEquals("testBar_(com.example.FooTest).exec", parser.text)
    }
  }

  @Test
  fun `manifest JSONL escapes special characters`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = TestableListener(outputDir, agent)

    val testName = "test\"quote\\slash\nline\rreturn\ttab\u0001control"
    listener.dumpForTest(testName, "complex")
    listener.close()

    val line = File(outputDir, "manifest.jsonl").readLines().single()
    assertTrue("Quotes should be escaped in raw JSON", line.contains("\\\"quote"))
    assertTrue("Backslashes should be escaped in raw JSON", line.contains("\\\\slash"))
    assertTrue("Newlines should be escaped in raw JSON", line.contains("\\nline"))
    assertTrue("Control characters should be escaped in raw JSON", line.contains("\\u0001control"))

    JsonFactory().createParser(line).use { parser ->
      parser.nextToken() // START_OBJECT
      parser.nextToken() // FIELD_NAME
      assertEquals("test_name", parser.currentName())
      parser.nextToken()
      assertEquals(testName, parser.text)
      parser.nextToken() // FIELD_NAME
      assertEquals("exec_file", parser.currentName())
      parser.nextToken()
      assertEquals("complex.exec", parser.text)
    }
  }

  @Test
  fun `bounds over-long exec filename within NAME_MAX`() {
    // Regression: long per-test names (descriptive method name + FQCN) overflowed the filesystem's
    // 255-byte per-component limit, so the .exec write failed with ENAMETOOLONG and surfaced as an
    // INFRA_FAILURE for the whole bundle.
    val outputDir = tempDir.newFolder("coverage")
    val listener = TestableListener(outputDir, FakeCoverageAgent(byteArrayOf(1, 2, 3)))

    val longName =
        "handleClick_calls_the_action_when_the_thing_is_in_the_right_state " +
            "(com.facebook.${"verylongpackagesegment.".repeat(8)}SomeVeryLongImplementationTest)"
    listener.dumpForTest(longName, longName)
    listener.close()

    assertNull("Over-long names must not error the bundle", listener.coverageError)

    val (testName, execName) =
        parseManifestLine(File(outputDir, "manifest.jsonl").readLines().single())
    assertTrue(
        "exec filename must fit NAME_MAX, got ${execName.toByteArray().size} bytes",
        execName.toByteArray().size <= 255,
    )
    assertTrue("exec file recorded in manifest must exist", File(outputDir, execName).exists())
    assertEquals("full test_name is preserved in the manifest", longName, testName)
  }

  @Test
  fun `over-long names sharing a prefix map to distinct exec files`() {
    val outputDir = tempDir.newFolder("coverage")
    val listener = TestableListener(outputDir, FakeCoverageAgent(byteArrayOf(1)))

    val prefix = "x".repeat(400)
    listener.dumpForTest("$prefix-alpha", "$prefix-alpha")
    listener.dumpForTest("$prefix-beta", "$prefix-beta")
    listener.close()

    val execNames =
        File(outputDir, "manifest.jsonl").readLines().map { parseManifestLine(it).second }
    assertEquals("Both dumps recorded", 2, execNames.size)
    assertEquals("Distinct exec files for distinct tests", 2, execNames.toSet().size)
  }

  /** Parses a manifest JSONL line into (test_name, exec_file), in the order the producer writes. */
  private fun parseManifestLine(line: String): Pair<String, String> {
    JsonFactory().createParser(line).use { parser ->
      parser.nextToken() // START_OBJECT
      parser.nextToken() // FIELD_NAME test_name
      parser.nextToken()
      val testName = parser.text
      parser.nextToken() // FIELD_NAME exec_file
      parser.nextToken()
      return testName to parser.text
    }
  }

  @Test
  fun `resets agent before each test`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = TestableListener(outputDir, agent)

    val desc1 = Description.createTestDescription("com.example.FooTest", "test1")
    val desc2 = Description.createTestDescription("com.example.FooTest", "test2")
    listener.testStarted(desc1)
    listener.testFinished(desc1)
    listener.testStarted(desc2)
    listener.testFinished(desc2)
    listener.close()

    assertEquals("Agent should be reset before each test", 2, agent.resetCount)
  }

  @Test
  fun `writes setup row at test run started`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(7))
    val listener = TestableListener(outputDir, agent)

    listener.testRunStarted(null)
    listener.close()

    assertTrue("Setup exec attributed to main", File(outputDir, "_setup.exec").exists())
  }

  @Test
  fun `writes teardown row at test run finished`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(9))
    val listener = TestableListener(outputDir, agent)

    listener.testRunFinished(null)
    listener.close()

    assertTrue("Teardown exec attributed to main", File(outputDir, "_teardown.exec").exists())
  }

  @Test
  fun `skips empty exec data`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf()) // empty
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testEmpty")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    assertEquals(
        "No exec files for empty coverage data",
        0,
        outputDir.listFiles { f -> f.name.endsWith(".exec") }?.size ?: 0,
    )
  }

  @Test
  fun `skips null exec data`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(null)
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testNull")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    assertEquals(
        "No exec files for null coverage data",
        0,
        outputDir.listFiles { f -> f.name.endsWith(".exec") }?.size ?: 0,
    )
  }

  @Test
  fun `sanitizes special characters in file name`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = TestableListener(outputDir, agent)

    // Parameterized tests can have spaces and special chars
    val desc = Description.createTestDescription("com.example.FooTest", "test[param1, param2]")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    val execFiles = outputDir.listFiles { f -> f.name.endsWith(".exec") }!!
    assertEquals(1, execFiles.size)
    assertEquals("test[param1,_param2]_(com.example.FooTest).exec", execFiles[0].name)
  }

  @Test
  fun `reset failure is captured in coverage error`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent =
        object : BasePerTestCoverageRunListener.CoverageAgent {
          override fun reset() {
            throw RuntimeException("agent unavailable")
          }

          override fun dumpAndReset(): ByteArray? = byteArrayOf(1)
        }
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testBar")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    assertEquals("agent unavailable", listener.coverageError?.message)
  }

  @Test
  fun `dump failure is captured in coverage error`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent =
        object : BasePerTestCoverageRunListener.CoverageAgent {
          override fun reset() {}

          override fun dumpAndReset(): ByteArray? {
            throw RuntimeException("dump failed")
          }
        }
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testBar")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    assertEquals("dump failed", listener.coverageError?.message)
    assertEquals(0, outputDir.listFiles { f -> f.name.endsWith(".exec") }?.size ?: 0)
  }

  @Test
  fun `does not set coverage error on success`() {
    val outputDir = tempDir.newFolder("coverage")
    val agent = FakeCoverageAgent(byteArrayOf(1))
    val listener = TestableListener(outputDir, agent)

    val desc = Description.createTestDescription("com.example.FooTest", "testBar")
    listener.testStarted(desc)
    listener.testFinished(desc)
    listener.close()

    assertNull(listener.coverageError)
  }

  /** Test-only subclass that exposes protected base behavior. */
  private class TestableListener(
      outputDir: File,
      agent: BasePerTestCoverageRunListener.CoverageAgent,
  ) : BasePerTestCoverageRunListener(outputDir, agent) {
    fun dumpForTest(testName: String, baseFileName: String) = dumpExec(testName, baseFileName)
  }

  /** Fake [BasePerTestCoverageRunListener.CoverageAgent] returning fixed data, tracking resets. */
  private class FakeCoverageAgent(private val execData: ByteArray?) :
      BasePerTestCoverageRunListener.CoverageAgent {
    var resetCount = 0
      private set

    override fun reset() {
      resetCount++
    }

    override fun dumpAndReset(): ByteArray? = execData
  }
}
