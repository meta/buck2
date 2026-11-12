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

import java.lang.reflect.Constructor
import java.lang.reflect.Method
import java.nio.file.Files
import java.nio.file.Path

object SourceTokenCounter {
  /**
   * Counts Kotlin tokens with the `KotlinLexer` that [compilerClassLoader] loads, the class loader
   * of the compiler that compiles the sources. Failures to load or run that lexer are thrown as
   * exceptions, never as errors.
   */
  fun countTokens(
      sourcePaths: List<Path>,
      rootPath: Path,
      compilerClassLoader: ClassLoader,
  ): TokenCounts {
    var kotlinTokens = 0L
    var javaTokens = 0L
    val kotlinLexer by lazy {
      try {
        CompilerKotlinLexer(compilerClassLoader)
      } catch (e: LinkageError) {
        throw IllegalStateException("Cannot load the compiler's KotlinLexer", e)
      } catch (e: ReflectiveOperationException) {
        throw IllegalStateException("Cannot load the compiler's KotlinLexer", e)
      }
    }
    for (path in sourcePaths) {
      val resolved = rootPath.resolve(path)
      val ext = path.toString().substringAfterLast('.', "")
      when (ext) {
        "kt",
        "kts" -> kotlinTokens += kotlinLexer.countTokens(Files.readString(resolved))
        "java" -> javaTokens += countJavaTokens(resolved)
      }
    }
    return TokenCounts(kotlinTokens, javaTokens)
  }

  private fun countJavaTokens(file: Path): Long {
    // Use StreamTokenizer for Java - lighter weight, no extra deps
    val text = Files.readString(file)
    val reader = java.io.StreamTokenizer(java.io.StringReader(text))
    reader.slashSlashComments(true)
    reader.slashStarComments(true)
    reader.ordinaryChar('/'.code)
    var count = 0L
    while (reader.nextToken() != java.io.StreamTokenizer.TT_EOF) {
      count++
    }
    return count
  }
}

/** Calls a compiler's `KotlinLexer` reflectively; whitespace and comments are not counted. */
private class CompilerKotlinLexer(classLoader: ClassLoader) {
  private val lexerClass: Class<*> =
      Class.forName("org.jetbrains.kotlin.lexer.KotlinLexer", true, classLoader)
  private val constructor: Constructor<*> = lexerClass.getConstructor()
  private val start: Method = lexerClass.getMethod("start", CharSequence::class.java)
  private val getTokenType: Method = lexerClass.getMethod("getTokenType")
  private val advance: Method = lexerClass.getMethod("advance")
  private val skippedTokenTypes: Set<Any> =
      Class.forName("org.jetbrains.kotlin.lexer.KtTokens", true, classLoader).let { tokens ->
        listOf("WHITE_SPACE", "EOL_COMMENT", "BLOCK_COMMENT", "DOC_COMMENT", "SHEBANG_COMMENT")
            .mapTo(HashSet()) { tokens.getField(it).get(null) }
      }

  fun countTokens(text: CharSequence): Long {
    val lexer = constructor.newInstance()
    start.invoke(lexer, text)
    var count = 0L
    while (true) {
      val tokenType = getTokenType.invoke(lexer) ?: return count
      if (tokenType !in skippedTokenTypes) {
        count++
      }
      advance.invoke(lexer)
    }
  }
}

data class TokenCounts(val kotlinTokens: Long, val javaTokens: Long) {
  val totalTokens: Long
    get() = kotlinTokens + javaTokens
}
