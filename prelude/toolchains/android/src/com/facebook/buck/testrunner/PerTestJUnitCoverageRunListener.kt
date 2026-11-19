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
import org.junit.runner.Description

/**
 * Per-test JaCoCo coverage `RunListener` for fbcode JVM (JUnit/Jupiter) tests.
 *
 * Reads the JaCoCo runtime agent via reflection on `org.jacoco.agent.rt.RT`. If the agent isn't on
 * the classpath (non-coverage builds), construction throws and the runner skips registration.
 *
 * Shared lifecycle (testRunStarted → `_setup`, testStarted → reset, testFinished → per-test dump,
 * testRunFinished → `_teardown`) lives in [BasePerTestCoverageRunListener]. Output format
 * (`{sanitized_test_name}.exec` files plus `manifest.jsonl`) is documented there.
 *
 * Activation: Set the `PER_TEST_COVERAGE_DIR` environment variable to the output directory.
 */
class PerTestJUnitCoverageRunListener
@JvmOverloads
constructor(
    outputDir: File,
    agent: CoverageAgent = ReflectiveCoverageAgent(),
    private val extensionManager: PerTestCoverageExtensionManager =
        PerTestCoverageExtensionManager.load(),
) : BasePerTestCoverageRunListener(outputDir, agent) {

  init {
    extensionManager.initialize(outputDir)
  }

  /** Default implementation that calls JaCoCo's `RT.getAgent()` via reflection. */
  class ReflectiveCoverageAgent : CoverageAgent {
    private val jacocoAgent: Any
    private val resetMethod: java.lang.reflect.Method
    private val getExecDataMethod: java.lang.reflect.Method

    init {
      val rtClass = Class.forName("org.jacoco.agent.rt.RT")
      jacocoAgent =
          rtClass.getMethod("getAgent").invoke(null)
              ?: throw IllegalStateException("JaCoCo agent returned null from RT.getAgent()")
      resetMethod = jacocoAgent::class.java.getMethod("reset")
      getExecDataMethod =
          jacocoAgent::class.java.getMethod("getExecutionData", Boolean::class.javaPrimitiveType)
    }

    override fun reset() {
      resetMethod.invoke(jacocoAgent)
    }

    override fun dumpAndReset(): ByteArray? {
      return getExecDataMethod.invoke(jacocoAgent, true) as? ByteArray
    }
  }

  // Per-testRun discriminator. JUnitRunner reuses one listener across multiple
  // jUnitCore.run() invocations (one per test class), and the suite's `description.displayName`
  // is the JUnit default "classes" for every invocation — so we tack on a counter to guarantee
  // unique setup/teardown exec filenames per run, otherwise they overwrite each other.
  private var runCount = 0

  override fun nextRunSuffix(description: Description?): String {
    runCount += 1
    val base = sanitizeFileName(description?.displayName ?: "anon")
    return "_${base}_$runCount"
  }

  override fun testStarted(description: Description) {
    super.testStarted(description)
    val testName = coverageTestName(description) ?: return
    extensionManager.testStarted(testName)
  }

  override fun testFinished(description: Description) {
    val testName = coverageTestName(description)
    if (testName != null) {
      extensionManager.testFinished(testName)
    }
    super.testFinished(description)
  }

  override fun close() {
    extensionManager.close()
    super.close()
  }
}
