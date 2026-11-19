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

import java.io.BufferedWriter
import java.io.Closeable
import java.io.File
import java.io.FileOutputStream
import java.io.FileWriter
import java.io.PrintWriter
import java.security.MessageDigest
import org.junit.runner.Description
import org.junit.runner.Result
import org.junit.runner.notification.RunListener

/**
 * Base class for per-test JaCoCo coverage `RunListener`s.
 *
 * Owns the shared lifecycle: setup and teardown dumps are attributed to the synthetic "main" row,
 * each test gets its own `.exec` file, and `manifest.jsonl` maps TPX test names to exec files.
 * Subclasses provide the [CoverageAgent], optional setup/teardown filename suffix, and any
 * cross-process error reporting.
 */
abstract class BasePerTestCoverageRunListener(
    protected val outputDir: File,
    protected val agent: CoverageAgent,
) : RunListener(), Closeable {

  /** Abstraction over JaCoCo's runtime agent for testability. */
  interface CoverageAgent {
    fun reset()

    fun dumpAndReset(): ByteArray?
  }

  /** First coverage error encountered, if any. Checked by the runner after test execution. */
  var coverageError: Exception? = null
    protected set

  private val manifest: PrintWriter
  private var runSuffix: String = ""

  init {
    check(outputDir.mkdirs() || outputDir.isDirectory) {
      "Failed to create per-test coverage output directory: $outputDir"
    }
    manifest = PrintWriter(BufferedWriter(FileWriter(File(outputDir, "manifest.jsonl"))), true)
  }

  override fun testRunStarted(description: Description?) {
    runSuffix = nextRunSuffix(description)
    // Snapshot probes accumulated before any individual @Test ran (framework boot, class loading,
    // static initializers, Application.onCreate on Android). Attributed to the synthetic "main"
    // TPX row. Without this, the next testStarted's reset() would silently wipe them.
    dumpExec(testName = "main", baseFileName = "_setup$runSuffix")
  }

  /**
   * Suffix appended to `_setup` / `_teardown` filenames. JVM overrides to disambiguate multiple
   * `jUnitCore.run()` invocations; Android uses the default single-run filenames.
   */
  protected open fun nextRunSuffix(description: Description?): String = ""

  override fun testStarted(description: Description) {
    try {
      agent.reset()
    } catch (e: Exception) {
      recordCoverageError("reset for ${description.displayName}", e)
    }
  }

  override fun testFinished(description: Description) {
    val testName = coverageTestName(description) ?: return
    dumpExec(testName = testName, baseFileName = testName)
  }

  /** TPX-format name shared with coverage extensions and `manifest.jsonl`. */
  protected fun coverageTestName(description: Description): String? {
    val className = description.className ?: return null
    val methodName = description.methodName ?: return null
    return "$methodName ($className)"
  }

  override fun testRunFinished(result: Result?) {
    // Probes that fire AFTER the last testFinished (framework teardown, deferred class loading) —
    // same bundle-level attribution as setup.
    // Do NOT close() the manifest here: the JVM runner reuses one listener instance across
    // multiple jUnitCore.run() invocations, so testRunFinished fires per class. Closing the
    // writer after the first class would silently drop later writes. Owners call close() once
    // after all classes are done.
    dumpExec(testName = "main", baseFileName = "_teardown$runSuffix")
  }

  override fun close() {
    manifest.close()
  }

  protected fun dumpExec(testName: String, baseFileName: String) {
    try {
      val execData = agent.dumpAndReset() ?: return
      if (execData.isEmpty()) return

      val fileName = boundedFileName(baseFileName, ".exec")
      val execFile = File(outputDir, fileName)
      FileOutputStream(execFile).use { fos -> fos.write(execData) }

      // Hand-rolled JSON to avoid a jackson-core dep that would duplicate fbandroid's on the APK.
      val jsonLine =
          "{\"test_name\":\"${escapeJsonString(testName)}\"," +
              "\"exec_file\":\"${escapeJsonString(fileName)}\"}"

      synchronized(manifest) { manifest.println(jsonLine) }
    } catch (e: Exception) {
      recordCoverageError("dump for $testName", e)
    }
  }

  private fun recordCoverageError(context: String, e: Exception) {
    if (coverageError != null) return
    coverageError = e
    onFirstCoverageError(context, e)
  }

  /**
   * Called after the first coverage error is stored. Android overrides this to write a sentinel
   * file; JVM callers can read [coverageError] directly.
   */
  protected open fun onFirstCoverageError(context: String, e: Exception) {}

  companion object {
    private val INVALID_FILENAME_CHARS = Regex("[/\\\\:*?\"<>| ]")

    /** Max bytes in a single path component on the filesystems we run on (Linux `NAME_MAX`). */
    private const val NAME_MAX_BYTES = 255

    fun sanitizeFileName(name: String): String = name.replace(INVALID_FILENAME_CHARS, "_")

    /**
     * Sanitized `<name><suffix>` filename bounded to [NAME_MAX_BYTES]. Per-test names (descriptive
     * method name + FQCN) routinely exceed the filesystem's per-component limit; without bounding,
     * the `.exec` write fails with `ENAMETOOLONG`, which the runner surfaces as an INFRA_FAILURE
     * for the whole bundle. Over-budget names fall back to a hash of the full name — short and
     * collision-free across tests. The chosen name is recorded in `manifest.jsonl`, so the daemon
     * reads whatever this returns — no cross-process agreement on the exact string is required.
     */
    fun boundedFileName(name: String, suffix: String): String {
      val sanitized = sanitizeFileName(name)
      val base =
          if (utf8Len(sanitized) + utf8Len(suffix) <= NAME_MAX_BYTES) sanitized else hashHex(name)
      return base + suffix
    }

    private fun utf8Len(s: String): Int = s.toByteArray(Charsets.UTF_8).size

    /** First 64 bits of SHA-256 as 16 hex chars; keeps over-long names collision-free. */
    private fun hashHex(s: String): String =
        MessageDigest.getInstance("SHA-256")
            .digest(s.toByteArray(Charsets.UTF_8))
            .take(8)
            .joinToString("") { "%02x".format(it.toInt() and 0xff) }

    private fun escapeJsonString(s: String): String {
      val sb = StringBuilder(s.length + 2)
      for (c in s) {
        when {
          c == '"' -> sb.append("\\\"")
          c == '\\' -> sb.append("\\\\")
          c == '\n' -> sb.append("\\n")
          c == '\r' -> sb.append("\\r")
          c == '\t' -> sb.append("\\t")
          c.code < 0x20 -> sb.append("\\u%04x".format(c.code))
          else -> sb.append(c)
        }
      }
      return sb.toString()
    }
  }
}
