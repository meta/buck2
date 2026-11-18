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

import java.io.IOException
import java.net.URI
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths

/**
 * Formats the compiler messages that the Build Tools API passes to a `KotlinLogger` the way the
 * kotlinc CLI prints them, which is what Buck's Kotlin action error handler and other tools parse:
 * `<path>:<line>:<column>: <severity>: <message>`, followed by the source line and a caret.
 *
 * Accepts the Build Tools API's own rendering, `<file URI>:<line>:<column> <Message>`, and returns
 * messages that are already in the CLI format unchanged. Paths under [workingDirectory] are printed
 * relative to it.
 */
internal class CompilerMessageFormatter(
    workingDirectory: Path = Paths.get("").toAbsolutePath(),
) {

  enum class Severity(val label: String) {
    ERROR("error"),
    EXCEPTION("exception"),
    WARNING("warning"),
  }

  private val workingDirectory: Path = workingDirectory.normalize()
  private val sourceLines = HashMap<Path, List<String>>()

  @Synchronized
  fun format(severity: Severity, message: String): String {
    val match = BTA_LOCATION.matchEntire(message)
    val path = match?.let { runCatching { Paths.get(URI(it.groupValues[1])) }.getOrNull() }
    if (match == null || path == null) {
      return if (CLI_MESSAGE.containsMatchIn(message)) message
      else "${severity.label}: ${decapitalize(message)}"
    }
    val (_, line, column, text) = match.destructured
    return buildString {
      append(relativize(path)).append(':')
      if (line.isNotEmpty()) append(line).append(':').append(column).append(':')
      append(' ').append(severity.label).append(": ").append(decapitalize(text))
      val lineNumber = line.toIntOrNull() ?: return@buildString
      val columnNumber = column.toIntOrNull() ?: return@buildString
      val lineContent = linesOf(path).getOrNull(lineNumber - 1) ?: return@buildString
      if (columnNumber > lineContent.length + 1) return@buildString
      append('\n').append(lineContent)
      append('\n').append(" ".repeat(columnNumber - 1)).append('^')
    }
  }

  private fun relativize(path: Path): Path {
    val normalized = path.normalize()
    return if (normalized.startsWith(workingDirectory)) workingDirectory.relativize(normalized)
    else path
  }

  private fun linesOf(path: Path): List<String> =
      sourceLines.getOrPut(path) {
        try {
          Files.readAllLines(path)
        } catch (_: IOException) {
          emptyList()
        }
      }

  private companion object {
    // `file:` URI, optional `:<line>:<column>`, a space, then the message.
    val BTA_LOCATION = Regex("""(file:\S+?)(?::(\d+):(\d+))? (.*)""", RegexOption.DOT_MATCHES_ALL)

    // An optional `<path>[:<line>[:<column>]]: ` location, then `<severity>: `.
    val CLI_MESSAGE = Regex("""^(?:[^\n]*?: )?(?:exception|error|warning): """)

    // Same rules as kotlinc's `PlainTextMessageRenderer`, so both renderings print the same text.
    fun decapitalize(message: String): String =
        when {
          message.startsWith("Java") || message.startsWith("Kotlin") -> message
          message.length >= 2 && message[0].isUpperCase() && message[1].isUpperCase() -> message
          message.isNotEmpty() && message[0] in 'A'..'Z' ->
              message[0].lowercaseChar() + message.substring(1)
          else -> message
        }
  }
}
