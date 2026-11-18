/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin

import com.facebook.buck.jvm.kotlin.ksp.KspInertShortcut
import java.io.File
import java.net.URLClassLoader
import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class KspInertShortcutTest {

  /** A shortcut that answers every step with one class resource. */
  object FakeShortcut {
    @JvmStatic
    fun inertOutputs(sources: List<String>, options: Map<String, String>): Map<String, Any?> =
        mapOf("outputs" to mapOf("res/answer.bin" to byteArrayOf(1, 2, 3)), "depFile" to null)
  }

  /** A shortcut that answers with one file in each of KSP2's three output directories. */
  object AllDirsShortcut {
    @JvmStatic
    fun inertOutputs(sources: List<String>, options: Map<String, String>): Map<String, Any?> =
        mapOf("outputs" to ALL_DIRS_OUTPUTS, "depFile" to null)
  }

  /** A shortcut that answers with two class resources and a dependency-file entry. */
  object TwoFilesShortcut {
    @JvmStatic
    fun inertOutputs(
        sources: List<String>,
        options: Map<String, String>,
    ): Map<String, Any?> = mapOf(
        "outputs" to linkedMapOf("res/a.bin" to byteArrayOf(1), "blocked/b.bin" to byteArrayOf(2)),
        "depFile" to "dep\n",
    )
  }

  /** A shortcut whose answer holds a value that is not bytes. */
  object WrongTypeShortcut {
    @JvmStatic
    fun inertOutputs(sources: List<String>, options: Map<String, String>): Map<String, Any?> =
        mapOf("outputs" to mapOf("res/a.bin" to "not bytes"), "depFile" to null)
  }

  /** A shortcut whose class fails to initialize. */
  object ThrowingInitShortcut {
    init {
      if (System.getProperty("never.set.by.tests") == null) {
        throw IllegalStateException("static init fails")
      }
    }

    @JvmStatic
    fun inertOutputs(sources: List<String>, options: Map<String, String>): Map<String, Any?> =
        emptyMap()
  }

  /** What [MissingDependencyShortcut] needs; [HidingLoader] makes it unloadable. */
  class Dependency {
    fun outputs(): Map<String, Any?> = emptyMap()
  }

  /** A shortcut whose answer needs a class that is missing at run time. */
  object MissingDependencyShortcut {
    @JvmStatic
    fun inertOutputs(sources: List<String>, options: Map<String, String>): Map<String, Any?> =
        Dependency().outputs()
  }

  /**
   * Defines [MissingDependencyShortcut] itself, from its class file, and cannot load [Dependency]:
   * the shortcut loads, and calling it raises `NoClassDefFoundError`.
   */
  private class HidingLoader(private val resources: ClassLoader, parent: ClassLoader) :
      ClassLoader(parent) {
    override fun loadClass(name: String, resolve: Boolean): Class<*> {
      synchronized(getClassLoadingLock(name)) {
        findLoadedClass(name)?.let {
          return it
        }
        if (name == Dependency::class.java.name) throw ClassNotFoundException(name)
        if (name == MissingDependencyShortcut::class.java.name) {
          val bytes =
              javaClass.classLoader.getResourceAsStream(name.replace('.', '/') + ".class")!!.use {
                it.readBytes()
              }
          return defineClass(name, bytes, 0, bytes.size)
        }
        return super.loadClass(name, resolve)
      }
    }

    override fun getResources(name: String) = resources.getResources(name)
  }

  private val provider = "com.example.Provider"

  private fun resourceDir(properties: String?): File {
    val dir = Files.createTempDirectory("ksp-inert-shortcut").toFile()
    if (properties != null) {
      val resource = File(dir, KspInertShortcut.RESOURCE)
      resource.parentFile.mkdirs()
      resource.writeText(properties)
    }
    return dir
  }

  private fun loaderWith(properties: String?): ClassLoader =
      URLClassLoader(arrayOf(resourceDir(properties).toURI().toURL()), javaClass.classLoader)

  private fun properties(mode: String, shortcut: Class<*> = FakeShortcut::class.java) =
      "provider=$provider\nshortcut=${shortcut.name}\nmode=$mode\n"

  private fun outputDirs(): KspInertShortcut.OutputDirs {
    val root = Files.createTempDirectory("ksp").toFile()
    return KspInertShortcut.OutputDirs(
        File(root, "classes"),
        File(root, "kotlin"),
        File(root, "java"),
    )
  }

  @Test
  fun `no resource means no shortcut`() {
    assertNull(KspInertShortcut.find(loaderWith(null), listOf(provider)))
  }

  @Test
  fun `each mode is read from the resource`() {
    for ((name, mode) in
        listOf(
            "skip" to KspInertShortcut.Mode.SKIP,
            "verify" to KspInertShortcut.Mode.VERIFY,
            "shadow" to KspInertShortcut.Mode.SHADOW,
        )) {
      assertEquals(
          mode,
          KspInertShortcut.find(loaderWith(properties(name)), listOf(provider))?.mode,
      )
    }
  }

  @Test
  fun `an unknown mode, another provider or several providers mean no shortcut`() {
    assertNull(KspInertShortcut.find(loaderWith(properties("fast")), listOf(provider)))
    assertNull(KspInertShortcut.find(loaderWith(properties("skip")), listOf("com.example.Other")))
    assertNull(
        KspInertShortcut.find(
            loaderWith(properties("skip")),
            listOf(provider, "com.example.Other"),
        ),
    )
  }

  @Test
  fun `compare finds equal outputs equal and reports every difference`() {
    val shortcut = KspInertShortcut.find(loaderWith(properties("shadow")), listOf(provider))!!
    val answer = shortcut.ask(emptyList(), emptyMap())
    val dirs = outputDirs()
    File(dirs.classes, "res").mkdirs()

    File(dirs.classes, "res/answer.bin").writeBytes(byteArrayOf(1, 2, 3))
    assertNull(shortcut.compare(answer, dirs, null, null).mismatch)

    File(dirs.classes, "res/answer.bin").writeBytes(byteArrayOf(1, 2, 4))
    assertNotNull(shortcut.compare(answer, dirs, null, null).mismatch)

    File(dirs.classes, "res/answer.bin").writeBytes(byteArrayOf(1, 2, 3))
    File(dirs.classes, "res/extra.bin").writeBytes(byteArrayOf(9))
    assertEquals(
        "only KSP wrote res/extra.bin (1 bytes)",
        shortcut.compare(answer, dirs, null, null).mismatch,
    )
  }

  @Test
  fun `skip writes each answer key to the output directory compare reads it from`() {
    val shortcut =
        KspInertShortcut.find(
            loaderWith(properties("skip", AllDirsShortcut::class.java)),
            listOf(provider),
        )!!
    val answer = shortcut.ask(emptyList(), emptyMap())
    val dirs = outputDirs()

    shortcut.write(answer, dirs, null)

    assertEquals("A", File(dirs.kotlin, "com/x/A.kt").readText())
    assertEquals("B", File(dirs.java, "com/x/B.java").readText())
    assertEquals("C", File(dirs.classes, "res/c.bin").readText())
    assertEquals(3, dirs.classes.parentFile.walkTopDown().count { it.isFile })
    // What skip wrote is exactly what a shadow comparison accepts.
    assertNull(shortcut.compare(answer, dirs, null, null).mismatch)
  }

  @Test
  fun `a shadow-ok answer is the one KSP2 wrote into its Kotlin and Java directories`() {
    val shortcut =
        KspInertShortcut.find(
            loaderWith(properties("shadow", AllDirsShortcut::class.java)),
            listOf(provider),
        )!!
    val answer = shortcut.ask(emptyList(), emptyMap())
    val dirs = outputDirs()
    // KSP2's own outputs.
    File(dirs.kotlin, "com/x").mkdirs()
    File(dirs.kotlin, "com/x/A.kt").writeText("A")
    File(dirs.java, "com/x").mkdirs()
    File(dirs.java, "com/x/B.java").writeText("B")
    File(dirs.classes, "res").mkdirs()
    File(dirs.classes, "res/c.bin").writeText("C")
    assertNull(shortcut.compare(answer, dirs, null, null).mismatch)

    // The same bytes in the classes directory are a different output.
    File(dirs.kotlin, "com/x/A.kt").delete()
    File(dirs.classes, "com/x").mkdirs()
    File(dirs.classes, "com/x/A.kt").writeText("A")
    assertNotNull(shortcut.compare(answer, dirs, null, null).mismatch)
  }

  @Test
  fun `a skip write that fails part-way removes what it wrote and leaves the report alone`() {
    val shortcut =
        KspInertShortcut.find(
            loaderWith(properties("skip", TwoFilesShortcut::class.java)),
            listOf(provider),
        )!!
    val answer = shortcut.ask(emptyList(), emptyMap())
    val dirs = outputDirs()
    // `blocked` is a regular file, so the second output cannot be written.
    dirs.classes.mkdirs()
    File(dirs.classes, "blocked").writeText("not a directory")
    val report = File(dirs.classes.parentFile, "report.txt").apply { writeText("before\n") }

    assertTrue(runCatching { shortcut.write(answer, dirs, report) }.isFailure)

    assertFalse(File(dirs.classes, "res/a.bin").exists())
    assertEquals("not a directory", File(dirs.classes, "blocked").readText())
    assertEquals("before\n", report.readText())
  }

  @Test
  fun `a skip write never overwrites an existing output and then writes nothing`() {
    val shortcut =
        KspInertShortcut.find(
            loaderWith(properties("skip", TwoFilesShortcut::class.java)),
            listOf(provider),
        )!!
    val answer = shortcut.ask(emptyList(), emptyMap())
    val dirs = outputDirs()
    File(dirs.classes, "blocked").mkdirs()
    File(dirs.classes, "blocked/b.bin").writeBytes(byteArrayOf(9))

    assertTrue(runCatching { shortcut.write(answer, dirs, null) }.isFailure)

    assertFalse(File(dirs.classes, "res/a.bin").exists())
    assertEquals(9.toByte(), File(dirs.classes, "blocked/b.bin").readBytes().single())
  }

  @Test
  fun `an answer value that is not bytes is a failed lookup`() {
    val loader = loaderWith(properties("skip", WrongTypeShortcut::class.java))
    val lookup = KspInertShortcut.lookup(loader, listOf(provider), emptyList(), emptyMap())
    assertTrue("$lookup", lookup is KspInertShortcut.Lookup.Failed)
  }

  @Test
  fun `an output key that would leave its directory is refused before anything is written`() {
    val dirs = outputDirs()
    for (key in
        listOf(
            "../escape.bin",
            "${KspInertShortcut.KOTLIN_SOURCES}../../escape.kt",
            "/abs.bin",
            "a//b",
            "",
        )) {
      assertThrows(key, IllegalArgumentException::class.java) { dirs.fileOf(key) }
    }
    assertFalse(
        dirs.classes.parentFile.exists() && dirs.classes.parentFile.walkTopDown().any { it.isFile },
    )
  }

  @Test
  fun `a shortcut class whose static init throws is a failed lookup, not a thrown error`() {
    val loader = loaderWith(properties("shadow", ThrowingInitShortcut::class.java))
    // The error a plain `catch (e: Exception)` around find would let through.
    val thrown =
        assertThrows(ExceptionInInitializerError::class.java) {
          KspInertShortcut.find(
              loaderWith(properties("shadow", ThrowingInitShortcut::class.java)),
              listOf(provider),
          )
        }
    // ExceptionInInitializerError is an Error: a `catch (e: Exception)` lets it through.
    assertTrue(Error::class.java.isAssignableFrom(thrown.javaClass))

    val lookup = KspInertShortcut.lookup(loader, listOf(provider), emptyList(), emptyMap())
    assertTrue(lookup is KspInertShortcut.Lookup.Failed)
  }

  @Test
  fun `a shortcut missing a dependency at run time is a failed lookup, not a thrown error`() {
    val resources = resourceDir(properties("shadow", MissingDependencyShortcut::class.java))
    val loader = HidingLoader(
        URLClassLoader(arrayOf(resources.toURI().toURL()), null),
        javaClass.classLoader,
    )
    val shortcut = KspInertShortcut.find(loader, listOf(provider))!!
    // The call fails with a linkage error (reflection reports it as the cause of an
    // InvocationTargetException).
    val thrown = runCatching { shortcut.ask(emptyList(), emptyMap()) }.exceptionOrNull()
    assertTrue("$thrown", thrown is NoClassDefFoundError || thrown?.cause is NoClassDefFoundError)

    val lookup = KspInertShortcut.lookup(loader, listOf(provider), emptyList(), emptyMap())
    assertTrue(lookup is KspInertShortcut.Lookup.Failed)
  }

  @Test
  fun `guarded returns the failure for any error but lets a virtual machine error through`() {
    assertEquals("ok", KspInertShortcut.guarded({ "failed" }) { "ok" })
    assertEquals("failed", KspInertShortcut.guarded({ "failed" }) { throw LinkageError("x") })
    assertEquals(
        "failed",
        KspInertShortcut.guarded({ "failed" }) { throw ExceptionInInitializerError("x") },
    )
    val oom = OutOfMemoryError("x")
    assertSame(
        oom,
        assertThrows(OutOfMemoryError::class.java) {
          KspInertShortcut.guarded({ "failed" }) { throw oom }
        },
    )
  }

  companion object {
    val ALL_DIRS_OUTPUTS = mapOf(
        "${KspInertShortcut.KOTLIN_SOURCES}com/x/A.kt" to "A".toByteArray(),
        "${KspInertShortcut.JAVA_SOURCES}com/x/B.java" to "B".toByteArray(),
        "res/c.bin" to "C".toByteArray(),
    )
  }
}
