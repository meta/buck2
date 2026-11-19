/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:JvmName("SkippedCompilationTrackerFactory")

package com.facebook.buck.jvm.kotlin.cd.workertool.postexecutors

import java.nio.file.Files
import java.nio.file.Path

/**
 * Tracks which .class files were not recompiled during incremental compilation.
 *
 * Pre-compilation modification timestamps are captured at construction time. After compilation,
 * [writeSkippedFiles] compares current timestamps to identify files that were not touched by the
 * compiler.
 */
sealed interface SkippedCompilationTracker {

  fun writeSkippedFiles()
}

internal class ActiveSkippedCompilationTracker(
    private val classesDir: Path,
    private val outputFile: Path,
) : SkippedCompilationTracker {

  private val preCompilationTimestamps: Map<String, Long> = getClassFileTimestamps(classesDir)

  override fun writeSkippedFiles() {
    val postCompilationTimestamps = getClassFileTimestamps(classesDir)
    val skippedFiles =
        postCompilationTimestamps.entries
            .filter { (path, mtime) -> preCompilationTimestamps[path] == mtime }
            .map { it.key }
            .sorted()

    outputFile.toFile().apply {
      parentFile?.mkdirs()
      writeText(
          if (skippedFiles.isNotEmpty()) skippedFiles.joinToString("\n") + "\n" else "",
      )
    }
  }
}

internal data object DoNothingSkippedCompilationTracker : SkippedCompilationTracker {

  override fun writeSkippedFiles() {
    // do nothing
  }
}

@JvmName("create")
fun SkippedCompilationTracker(
    outputFile: Path?,
    classesDir: Path?,
): SkippedCompilationTracker {
  if (outputFile == null || classesDir == null) {
    return DoNothingSkippedCompilationTracker
  }

  return ActiveSkippedCompilationTracker(classesDir, outputFile)
}

private fun getClassFileTimestamps(dir: Path): Map<String, Long> {
  if (!Files.isDirectory(dir)) return emptyMap()

  val timestamps = mutableMapOf<String, Long>()
  Files.walk(dir).use { stream ->
    stream
        .filter { Files.isRegularFile(it) && it.toString().endsWith(".class") }
        .forEach { file ->
          val relPath = dir.relativize(file).toString()
          timestamps[relPath] = Files.getLastModifiedTime(file).toMillis()
        }
  }
  return timestamps
}
