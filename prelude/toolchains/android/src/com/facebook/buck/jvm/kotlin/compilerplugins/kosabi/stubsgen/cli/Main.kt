/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.cli

import java.io.File
import kotlin.system.exitProcess

private const val USAGE =
    """Usage: stubsgen-cli --sources <path>[,<path>...] [options]

Runs Kosabi stub generation on a set of Kotlin and Java sources using a minimal, parse-only
PSI environment (no kotlinc / FIR / codegen / classpath index).

Required:
  --sources <path>[,<path>...]   Comma-separated source files and/or directories
                                 (directories are searched recursively for .kt/.kts/.java).

At least one output must be given:
  --stubs-out <dir>              Directory for generated .kt stubs.
  --classes-out <dir>           Directory for generated .class (bytecode) stubs.

Optional:
  --classpath <path>[:<path>...] Path-separator-separated jars/dirs whose already-present
                                 classes should NOT be stubbed. Default: empty.
  --log <path>                   Write the stubsgen log to this file.
  --skip-java-imports <bool>     Collect Java declarations only, skipping the javac
                                 import parse (for modules without KSP processors).

Exit codes: 0 success, 1 generation failure, 2 usage error."""

private const val EXIT_OK = 0
private const val EXIT_FAILURE = 1
private const val EXIT_USAGE = 2

private class CliArgs(
    val sources: List<File>,
    val classpath: List<File>,
    val stubsDumpDir: File?,
    val stubsClassOutputDir: File?,
    val logPath: String?,
    val skipJavaImports: Boolean,
)

private fun parseArgs(args: Array<String>): CliArgs? {
  var sourcesArg: String? = null
  var stubsOut: String? = null
  var classesOut: String? = null
  var classpathArg: String? = null
  var logPath: String? = null
  var skipJavaImports = false

  var i = 0
  while (i < args.size) {
    val key = args[i]
    if (!key.startsWith("--")) {
      System.err.println("Unexpected argument: $key")
      return null
    }
    if (i + 1 >= args.size) {
      System.err.println("Missing value for $key")
      return null
    }
    val value = args[i + 1]
    if (value.startsWith("--")) {
      // The next token is another flag, not a value — report a clear error instead of
      // silently consuming it (e.g. `--sources --stubs-out x`).
      System.err.println("Missing value for $key")
      return null
    }
    when (key) {
      "--sources" -> sourcesArg = value
      "--stubs-out" -> stubsOut = value
      "--classes-out" -> classesOut = value
      "--classpath" -> classpathArg = value
      "--log" -> logPath = value
      "--skip-java-imports" -> skipJavaImports = value.toBoolean()
      else -> {
        System.err.println("Unknown flag: $key")
        return null
      }
    }
    i += 2
  }

  if (sourcesArg == null) {
    System.err.println("Missing required --sources")
    return null
  }
  val sources = collectSourceFiles(sourcesArg) ?: return null
  if (sources.isEmpty()) {
    System.err.println("No .kt/.kts/.java sources found under: $sourcesArg")
    return null
  }

  val stubsDumpDir = stubsOut?.let { File(it) }
  val stubsClassOutputDir = classesOut?.let { File(it) }
  if (stubsDumpDir == null && stubsClassOutputDir == null) {
    System.err.println("At least one of --stubs-out / --classes-out is required")
    return null
  }

  val classpath =
      classpathArg?.split(File.pathSeparatorChar)?.filter { it.isNotBlank() }?.map { File(it) }
          ?: emptyList()

  return CliArgs(
      sources = sources,
      classpath = classpath,
      stubsDumpDir = stubsDumpDir,
      stubsClassOutputDir = stubsClassOutputDir,
      logPath = logPath,
      skipJavaImports = skipJavaImports,
  )
}

private fun collectSourceFiles(sourcesArg: String): List<File>? {
  // --sources is comma-delimited; source paths must therefore not contain a literal comma.
  val entries = sourcesArg.split(',').map { it.trim() }.filter { it.isNotEmpty() }

  val collected = mutableListOf<File>()
  for (entry in entries) {
    val path = File(entry)
    when {
      path.isDirectory ->
          collected += path.walkTopDown().filter { it.isFile && it.isSupportedSource() }
      path.isFile && path.isSupportedSource() -> collected += path
      path.isFile -> {
        System.err.println("Not a Kotlin or Java source (expected .kt/.kts/.java): $entry")
        return null
      }
      else -> {
        System.err.println("Source path does not exist: $entry")
        return null
      }
    }
  }

  return collected.sortedBy { it.absolutePath }
}

private fun File.isSupportedSource(): Boolean =
    extension == "kt" || extension == "kts" || extension == "java"

fun main(args: Array<String>) {
  val parsed = parseArgs(args)
  if (parsed == null) {
    System.err.println()
    System.err.println(USAGE)
    exitProcess(EXIT_USAGE)
  }

  val exitCode =
      try {
        StubsGenCli.generate(
            sourceFiles = parsed.sources,
            classpath = parsed.classpath,
            stubsDumpDir = parsed.stubsDumpDir,
            stubsClassOutputDir = parsed.stubsClassOutputDir,
            logPath = parsed.logPath,
            skipJavaImports = parsed.skipJavaImports,
        )
        EXIT_OK
      } catch (t: Throwable) {
        System.err.println("stubsgen-cli: stub generation failed")
        t.printStackTrace()
        EXIT_FAILURE
      }

  exitProcess(exitCode)
}
