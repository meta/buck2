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
import java.util.ServiceLoader
import java.util.concurrent.ConcurrentHashMap

/** Manages discovery and lifecycle callbacks for optional per-test coverage extensions. */
class PerTestCoverageExtensionManager(
    private val extensions: List<PerTestCoverageExtension>,
) : PerTestCoverageExtension {

  private val reportedFailures = ConcurrentHashMap.newKeySet<String>()

  override fun initialize(outputDir: File) {
    extensions.forEach { extension ->
      runExtension(extension, "initialization") { extension.initialize(outputDir) }
    }
  }

  override fun testStarted(testName: String) {
    extensions.forEach { extension ->
      runExtension(extension, "test start for $testName") { extension.testStarted(testName) }
    }
  }

  override fun testFinished(testName: String) {
    extensions.forEach { extension ->
      runExtension(extension, "test finish for $testName") { extension.testFinished(testName) }
    }
  }

  override fun close() {
    extensions.asReversed().forEach { extension ->
      runExtension(extension, "close") { extension.close() }
    }
  }

  private inline fun runExtension(
      extension: PerTestCoverageExtension,
      context: String,
      block: () -> Unit,
  ) {
    try {
      block()
    } catch (failure: Throwable) {
      if (isFatal(failure)) throw failure
      if (reportedFailures.add(extension.javaClass.name)) {
        logFailure(extension.javaClass.name, context, failure)
      }
    }
  }

  companion object {
    fun load(): PerTestCoverageExtensionManager =
        try {
          load(ServiceLoader.load(PerTestCoverageExtension::class.java).iterator())
        } catch (failure: Throwable) {
          if (isFatal(failure)) throw failure
          logFailure(
              PerTestCoverageExtension::class.java.name,
              "provider discovery",
              failure,
          )
          PerTestCoverageExtensionManager(emptyList())
        }

    internal fun load(
        providers: Iterator<PerTestCoverageExtension>,
    ): PerTestCoverageExtensionManager {
      val extensions = ArrayList<PerTestCoverageExtension>()
      while (true) {
        try {
          if (!providers.hasNext()) break
          extensions.add(providers.next())
        } catch (failure: Throwable) {
          if (isFatal(failure)) throw failure
          logFailure(
              PerTestCoverageExtension::class.java.name,
              "provider discovery",
              failure,
          )
        }
      }
      return PerTestCoverageExtensionManager(extensions)
    }

    private fun logFailure(
        extensionName: String,
        context: String,
        failure: Throwable,
    ) {
      System.err.println(
          "Per-test coverage extension $extensionName failed during $context; " +
              "continuing primary coverage without failing the test run.",
      )
      failure.printStackTrace(System.err)
    }

    private fun isFatal(failure: Throwable): Boolean =
        failure is VirtualMachineError || failure is ThreadDeath
  }
}
