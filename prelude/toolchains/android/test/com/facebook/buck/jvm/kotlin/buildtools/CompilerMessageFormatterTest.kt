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

import com.facebook.buck.jvm.kotlin.buildtools.CompilerMessageFormatter.Severity
import com.facebook.buck.testutil.TemporaryPaths
import java.nio.file.Files
import java.nio.file.Path
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Rule
import org.junit.Test

internal class CompilerMessageFormatterTest {

  @JvmField @Rule val temporaryFolder: TemporaryPaths = TemporaryPaths(false)

  private lateinit var root: Path
  private lateinit var source: Path
  private lateinit var formatter: CompilerMessageFormatter

  @Before
  fun setUp() {
    root = temporaryFolder.root.path
    source = Files.createDirectories(root.resolve("src")).resolve("Foo.kt")
    Files.writeString(source, "fun f(): Int {\n  return \"s\"\n}\n")
    formatter = CompilerMessageFormatter(root)
  }

  @Test
  fun `build tools api rendering is formatted like the kotlinc CLI`() {
    assertEquals(
        "src/Foo.kt:2:10: error: return type mismatch: expected 'Int', actual 'String'.\n" +
            "  return \"s\"\n" +
            "         ^",
        formatter.format(
            Severity.ERROR,
            "${source.toUri()}:2:10 Return type mismatch: expected 'Int', actual 'String'.",
        ),
    )
  }

  @Test
  fun `kotlinc CLI rendering of the same diagnostic is unchanged`() {
    val cli = "src/Foo.kt:2:10: error: unresolved reference 's'.\n  return \"s\"\n         ^"
    assertEquals(cli, formatter.format(Severity.ERROR, cli))
    assertEquals(
        cli,
        formatter.format(Severity.ERROR, "${source.toUri()}:2:10 Unresolved reference 's'."),
    )
    assertEquals(
        "error: no source files",
        formatter.format(Severity.ERROR, "error: no source files"),
    )
    assertEquals("warning: w", formatter.format(Severity.WARNING, "warning: w"))
    assertEquals(
        "Foo.kt: exception: e",
        formatter.format(Severity.EXCEPTION, "Foo.kt: exception: e"),
    )
  }

  @Test
  fun `messages without a location get a severity`() {
    assertEquals(
        "error: warnings found and -Werror specified",
        formatter.format(Severity.ERROR, "Warnings found and -Werror specified"),
    )
    assertEquals("warning: Kotlin x", formatter.format(Severity.WARNING, "Kotlin x"))
    assertEquals("warning: JVM x", formatter.format(Severity.WARNING, "JVM x"))
    assertEquals("exception: e", formatter.format(Severity.EXCEPTION, "e"))
  }

  @Test
  fun `locations without a line, outside the working directory, or past the line are kept`() {
    assertEquals(
        "src/Foo.kt: warning: w",
        formatter.format(Severity.WARNING, "${source.toUri()} W"),
    )
    val outside = CompilerMessageFormatter(root.resolve("other"))
    assertEquals("$source:9:1: error: e", outside.format(Severity.ERROR, "${source.toUri()}:9:1 E"))
    assertEquals(
        "src/Foo.kt:1:16: error: e",
        formatter.format(Severity.ERROR, "${source.toUri()}:1:16 E"),
    )
    assertEquals(
        "src/Missing.kt:1:1: error: e",
        formatter.format(Severity.ERROR, "${root.resolve("src/Missing.kt").toUri()}:1:1 E"),
    )
  }
}
