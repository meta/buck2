/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.analytics

import java.nio.file.Path
import org.jetbrains.kotlin.lexer.KotlinLexer
import org.jetbrains.kotlin.lexer.KtTokens
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

internal class SourceTokenCounterTest {

  @get:Rule val tempDir = TemporaryFolder()

  private val compilerClassLoader = SourceTokenCounterTest::class.java.classLoader

  @Test
  fun `countTokens returns zero for empty source list`() {
    val result =
        SourceTokenCounter.countTokens(emptyList(), tempDir.root.toPath(), compilerClassLoader)
    assertEquals(0L, result.kotlinTokens)
    assertEquals(0L, result.javaTokens)
    assertEquals(0L, result.totalTokens)
  }

  @Test
  fun `countTokens counts Kotlin tokens`() {
    val ktFile = tempDir.newFile("Test.kt")
    ktFile.writeText("fun main() { println(\"hello\") }")
    val sourcePaths = listOf(Path.of("Test.kt"))

    val result =
        SourceTokenCounter.countTokens(sourcePaths, tempDir.root.toPath(), compilerClassLoader)

    assert(result.kotlinTokens > 0) { "Expected positive Kotlin token count" }
    assertEquals(0L, result.javaTokens)
    assertEquals(result.kotlinTokens, result.totalTokens)
  }

  @Test
  fun `countTokens counts Java tokens`() {
    val javaFile = tempDir.newFile("Test.java")
    javaFile.writeText("public class Test { public static void main(String[] args) {} }")
    val sourcePaths = listOf(Path.of("Test.java"))

    val result =
        SourceTokenCounter.countTokens(sourcePaths, tempDir.root.toPath(), compilerClassLoader)

    assertEquals(0L, result.kotlinTokens)
    assert(result.javaTokens > 0) { "Expected positive Java token count" }
    assertEquals(result.javaTokens, result.totalTokens)
  }

  @Test
  fun `countTokens counts both Kotlin and Java files`() {
    val ktFile = tempDir.newFile("Test.kt")
    ktFile.writeText("val x = 1")
    val javaFile = tempDir.newFile("Test.java")
    javaFile.writeText("class Test { int x = 1; }")
    val sourcePaths = listOf(Path.of("Test.kt"), Path.of("Test.java"))

    val result =
        SourceTokenCounter.countTokens(sourcePaths, tempDir.root.toPath(), compilerClassLoader)

    assert(result.kotlinTokens > 0) { "Expected positive Kotlin token count" }
    assert(result.javaTokens > 0) { "Expected positive Java token count" }
    assertEquals(result.kotlinTokens + result.javaTokens, result.totalTokens)
  }

  @Test
  fun `countTokens handles kts extension`() {
    val ktsFile = tempDir.newFile("build.gradle.kts")
    ktsFile.writeText("plugins { id(\"java\") }")
    val sourcePaths = listOf(Path.of("build.gradle.kts"))

    val result =
        SourceTokenCounter.countTokens(sourcePaths, tempDir.root.toPath(), compilerClassLoader)

    assert(result.kotlinTokens > 0) { "Expected positive Kotlin token count for .kts file" }
  }

  @Test
  fun `countTokens ignores non-kotlin-non-java files`() {
    val txtFile = tempDir.newFile("readme.txt")
    txtFile.writeText("this is not source code")
    val sourcePaths = listOf(Path.of("readme.txt"))

    val result =
        SourceTokenCounter.countTokens(sourcePaths, tempDir.root.toPath(), compilerClassLoader)

    assertEquals(0L, result.kotlinTokens)
    assertEquals(0L, result.javaTokens)
  }

  @Test
  fun `countTokens excludes whitespace and comments from Kotlin token count`() {
    val ktFile = tempDir.newFile("Commented.kt")

    // Code without comments
    ktFile.writeText("val x = 1")
    val baseResult =
        SourceTokenCounter.countTokens(
            listOf(Path.of("Commented.kt")),
            tempDir.root.toPath(),
            compilerClassLoader,
        )

    // Same code with added comments and extra whitespace — should yield same count
    ktFile.writeText(
        "// line comment\nval   x   =   1 /* block comment */ /** doc comment */\n// trailing",
    )
    val commentedResult =
        SourceTokenCounter.countTokens(
            listOf(Path.of("Commented.kt")),
            tempDir.root.toPath(),
            compilerClassLoader,
        )

    assertEquals(baseResult.kotlinTokens, commentedResult.kotlinTokens)
  }

  @Test
  fun `countTokens handles empty source file`() {
    val ktFile = tempDir.newFile("Empty.kt")
    ktFile.writeText("")
    val sourcePaths = listOf(Path.of("Empty.kt"))

    val result =
        SourceTokenCounter.countTokens(sourcePaths, tempDir.root.toPath(), compilerClassLoader)

    assertEquals(0L, result.kotlinTokens)
  }

  @Test
  fun `countTokens counts the tokens that KotlinLexer returns, without whitespace and comments`() {
    val text =
        "#!/usr/bin/env kotlin\npackage a\n\n/** doc */\nfun f(args: Array<String>) {\n" +
            "  val s = \"x${'$'}{args.size}\" /* block */ // line\n}\n"
    tempDir.newFile("Lexed.kt").writeText(text)
    val skipped = setOf(
        KtTokens.WHITE_SPACE,
        KtTokens.EOL_COMMENT,
        KtTokens.BLOCK_COMMENT,
        KtTokens.DOC_COMMENT,
        KtTokens.SHEBANG_COMMENT,
    )
    val lexer = KotlinLexer()
    lexer.start(text)
    var expected = 0L
    while (lexer.tokenType != null) {
      if (lexer.tokenType !in skipped) {
        expected++
      }
      lexer.advance()
    }

    val result =
        SourceTokenCounter.countTokens(
            listOf(Path.of("Lexed.kt")),
            tempDir.root.toPath(),
            compilerClassLoader,
        )

    assertEquals(expected, result.kotlinTokens)
  }

  @Test
  fun `countTokens throws an exception when the compiler's lexer cannot be loaded`() {
    tempDir.newFile("A.kt").writeText("val x = 1")
    val brokenLoader =
        object : ClassLoader(null) {
          override fun loadClass(name: String, resolve: Boolean): Class<*> =
              throw NoClassDefFoundError(name)
        }

    assertThrows(IllegalStateException::class.java) {
      SourceTokenCounter.countTokens(listOf(Path.of("A.kt")), tempDir.root.toPath(), brokenLoader)
    }
  }

  @Test
  fun `countTokens wraps reflection failures when the compiler's lexer API drifts`() {
    tempDir.newFile("A.kt").writeText("val x = 1")
    // A class that loads but lacks the expected members, so member lookup throws
    // NoSuchMethodException (a ReflectiveOperationException) instead of a LinkageError.
    val driftingLoader =
        object : ClassLoader(null) {
          override fun loadClass(name: String, resolve: Boolean): Class<*> = Integer::class.java
        }

    val e =
        assertThrows(IllegalStateException::class.java) {
          SourceTokenCounter.countTokens(
              listOf(Path.of("A.kt")),
              tempDir.root.toPath(),
              driftingLoader,
          )
        }
    assertEquals("Cannot load the compiler's KotlinLexer", e.message)
  }
}
