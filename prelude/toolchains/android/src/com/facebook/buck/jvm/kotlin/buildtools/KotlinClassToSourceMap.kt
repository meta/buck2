/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.buildtools

import com.facebook.buck.core.util.log.Logger
import com.facebook.buck.jvm.java.CompilerClassToSourceMap
import java.io.IOException
import java.nio.file.NoSuchFileException
import java.nio.file.Path
import org.jetbrains.kotlin.buildtools.api.KotlinLogger

internal class KotlinClassToSourceMap(
    private val output: Path,
    private val classesDir: Path,
    private val sourceRoot: Path,
    private val state: Path? = null,
) {
  private val restoredMapping = state?.let {
    try {
      CompilerClassToSourceMap.read(it)
    } catch (_: NoSuchFileException) {
      LOG.debug("No source mapping state at %s; forcing a full compilation", it)
      null
    } catch (error: IOException) {
      LOG.warn(error, "Cannot restore source mapping state from %s; forcing a full compilation", it)
      null
    }
  }
  val requiresRebuild: Boolean = state != null && restoredMapping == null
  private val mapping = restoredMapping ?: CompilerClassToSourceMap()

  fun logger(delegate: KotlinLogger): KotlinLogger =
      object : KotlinLogger by delegate {
        override val isDebugEnabled: Boolean = true

        override fun debug(msg: String) {
          if (!recordOutput(msg)) delegate.debug(msg)
        }
      }

  fun recordOutput(message: String): Boolean {
    val report = message.removePrefix("output: ")
    if (!report.startsWith("Output:\n", ignoreCase = true)) return false
    val lines = report.lines()
    if (lines.size < 3 || lines[2] != "Sources:") return false
    val classFile = Path.of(lines[1]).toAbsolutePath().normalize()
    val root = classesDir.toAbsolutePath().normalize()
    if (classFile.startsWith(root) && classFile.toString().endsWith(".class")) {
      mapping.put(
          root.relativize(classFile).toString(),
          lines.drop(3).filter { it.isNotEmpty() }.map { Path.of(it) },
          sourceRoot,
      )
    }
    return true
  }

  fun write() {
    mapping.retainExistingOutputs(classesDir)
    mapping.write(output)
    if (state != null) mapping.write(state)
  }

  companion object {
    private val LOG: Logger = Logger.get(KotlinClassToSourceMap::class.java)
  }
}
