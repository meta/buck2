/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util

import java.io.ByteArrayInputStream
import java.io.DataInputStream
import java.io.File
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.jar.JarFile

/**
 * Answers whether a `pkg.Name` the sources import is already provided by the stubsgen classpath, so
 * stub generation does not fabricate a class that shadows it.
 *
 * Two classpath shapes defeat the entry-name scan in `parseClasspathFileClassesAndPackages`: jars
 * on its skip list (e.g. kotlinx-coroutines-core-jvm, whose real classes would otherwise gain empty
 * same-FQN phantom stubs), and Kotlin typealiases, which have no class file at all (e.g.
 * `kotlinx.coroutines.Runnable` -> `java.lang.Runnable`; a phantom class stub carries a descriptor
 * no consumer can link against). Both checks read from the jars directly and cache per instance;
 * jars are never modified.
 */
class KotlinClasspathDeclarations(private val classpath: List<File>) {
  private val jarEntriesCache = mutableMapOf<String, Set<String>>()
  private val facadeNamesCache = mutableMapOf<String, Set<String>>()

  /**
   * True when `pkgSegments.Name` is provided by the classpath: either `pkg/Name.class` exists in
   * some entry (including skip-listed jars), or `Name` is a Kotlin top-level declaration recorded
   * in a file facade of the package. Callers must not fabricate a class stub then.
   */
  fun isProvided(pkgSegments: List<String>, name: String): Boolean {
    if (pkgSegments.isEmpty() || name.isEmpty()) return false
    val pkgPath = pkgSegments.joinToString("/")
    return hasClassEntry(pkgPath, name) || hasKotlinTopLevelName(pkgPath, name)
  }

  private fun entriesOf(jar: File): Set<String> {
    val key = jar.path
    return jarEntriesCache.getOrPut(key) {
      try {
        JarFile(jar).use { jarFile -> jarFile.entries().asSequence().map { it.name }.toSet() }
      } catch (e: Exception) {
        emptySet()
      }
    }
  }

  private fun hasClassEntry(pkgPath: String, name: String): Boolean {
    val entry = "$pkgPath/$name.class"
    return classpath.any { jar -> entriesOf(jar).contains(entry) }
  }

  private fun hasKotlinTopLevelName(pkgPath: String, name: String): Boolean {
    for (jar in classpath) {
      val entries = entriesOf(jar)
      // Exact-package match only: a `startsWith` prefix also matches facades in
      // subpackages, whose same-named declarations belong to another package and
      // must not mark the import provided.
      val facades = entries.filter {
        it.substringBeforeLast('/') == pkgPath && isLikelyFileFacade(it)
      }
      for (facade in facades) {
        if (facadeD2Names(jar, facade).contains(name)) return true
      }
    }
    return false
  }

  private fun isLikelyFileFacade(entry: String): Boolean {
    if (!entry.endsWith(".class")) return false
    val simple = entry.substringAfterLast('/')
    // File facades (FooKt, including multifile parts FooKt__BarKt). Custom @JvmName facades
    // do not match; they stay stub-eligible, which fails loud rather than wrong.
    return simple.endsWith("Kt.class")
  }

  private fun facadeD2Names(jar: File, entry: String): Set<String> {
    val key = "${jar.path}!$entry"
    return facadeNamesCache.getOrPut(key) {
      try {
        JarFile(jar).use { jarFile ->
          val bytes = jarFile.getInputStream(jarFile.getEntry(entry)).readBytes()
          readMetadataD2(bytes)
        }
      } catch (e: Exception) {
        emptySet()
      }
    }
  }

  /**
   * Minimal class-file reader returning the kotlin.Metadata `d2` string array. Only the constant
   * pool and the class-level Runtime(In)visibleAnnotations are walked; anything unexpected yields
   * an empty set so stub generation keeps its previous behavior.
   */
  internal fun readMetadataD2(classBytes: ByteArray): Set<String> {
    try {
      val buf = ByteBuffer.wrap(classBytes).order(ByteOrder.BIG_ENDIAN)
      if (buf.int != 0xCAFEBABE.toInt()) return emptySet()
      buf.getShort() // minor
      buf.getShort() // major
      val cpCount = buf.getShort().toInt() and 0xFFFF
      val utf8 = arrayOfNulls<String>(cpCount)
      var i = 1
      while (i < cpCount) {
        when (val tag = buf.get().toInt() and 0xFF) {
          1 -> {
            val len = buf.getShort().toInt() and 0xFFFF
            val bytes = ByteArray(len)
            buf.get(bytes)
            // Class-file strings are modified UTF-8; the standard decoder mangles
            // supplementary characters, so decode through readUTF.
            utf8[i] =
                DataInputStream(
                    ByteArrayInputStream(byteArrayOf((len ushr 8).toByte(), len.toByte()) + bytes),
                )
                    .readUTF()
          }
          7,
          8,
          16,
          19,
          20 -> buf.getShort()
          15 -> {
            buf.get()
            buf.getShort()
          }
          9,
          10,
          11,
          12,
          17,
          18 -> buf.int
          3,
          4 -> buf.int
          5,
          6 -> {
            buf.long
            i++
          }
          else -> return emptySet()
        }
        i++
      }
      fun utf(idx: Int): String? = if (idx in 1 until cpCount) utf8[idx] else null
      buf.getShort() // access_flags
      buf.getShort() // this_class
      buf.getShort() // super_class
      repeat(buf.getShort().toInt() and 0xFFFF) { buf.getShort() } // interfaces
      repeat(buf.getShort().toInt() and 0xFFFF) { // fields
        repeat(3) { buf.getShort() }
        repeat(buf.getShort().toInt() and 0xFFFF) { // attributes
          buf.getShort()
          val len = buf.int
          buf.position(buf.position() + len)
        }
      }
      repeat(buf.getShort().toInt() and 0xFFFF) { // methods
        repeat(3) { buf.getShort() }
        repeat(buf.getShort().toInt() and 0xFFFF) { // attributes
          buf.getShort()
          val len = buf.int
          buf.position(buf.position() + len)
        }
      }
      repeat(buf.getShort().toInt() and 0xFFFF) { // class attributes
        val nameIdx = buf.getShort().toInt() and 0xFFFF
        val len = buf.int
        val end = buf.position() + len
        val attrName = utf(nameIdx)
        if (attrName == "RuntimeInvisibleAnnotations" || attrName == "RuntimeVisibleAnnotations") {
          val numAnnotations = buf.getShort().toInt() and 0xFFFF
          repeat(numAnnotations) {
            val typeName = utf(buf.getShort().toInt() and 0xFFFF)
            val numPairs = buf.getShort().toInt() and 0xFFFF
            val pairs = mutableMapOf<String, Any>()
            repeat(numPairs) {
              val elemName = utf(buf.getShort().toInt() and 0xFFFF) ?: ""
              pairs[elemName] = readElementValue(buf, ::utf)
            }
            if (typeName == "Lkotlin/Metadata;") {
              @Suppress("UNCHECKED_CAST") val d2 = pairs["d2"] as? List<String> ?: emptyList()
              return d2.toSet()
            }
          }
        }
        buf.position(end)
      }
    } catch (e: Exception) {
      return emptySet()
    }
    return emptySet()
  }

  private fun readElementValue(buf: ByteBuffer, utf: (Int) -> String?): Any {
    return when (val tag = buf.get().toInt().toChar()) {
      's' -> utf(buf.getShort().toInt() and 0xFFFF) ?: ""
      '[' -> {
        val n = buf.getShort().toInt() and 0xFFFF
        List(n) { readElementValue(buf, utf) }
      }
      'c' -> utf(buf.getShort().toInt() and 0xFFFF) ?: ""
      'e' -> {
        buf.getShort()
        buf.getShort()
        ""
      }
      '@' -> {
        // Nested annotation: skip type + pairs without interpreting them.
        buf.getShort()
        val n = buf.getShort().toInt() and 0xFFFF
        repeat(n) {
          buf.getShort()
          readElementValue(buf, utf)
        }
        ""
      }
      else -> {
        // Primitive constant (2-byte index) or anything unexpected: consume the index.
        buf.getShort()
        ""
      }
    }
  }
}
