/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.ksp

import java.io.File
import java.nio.charset.StandardCharsets
import java.util.Properties

/**
 * Lets a target's only KSP processor answer before KSP2 starts.
 *
 * A processor jar opts in with a `META-INF/kotlincd/ksp-inert-shortcut` properties resource:
 * `provider` (its `SymbolProcessorProvider`), `shortcut` (a class with a static
 * `inertOutputs(List<String> sources, Map<String, String> options): Map<String, Object>`) and
 * `mode`. The shortcut either declines (`declined` holds why; KSP runs as before) or returns the
 * exact files KSP would write (`outputs`, keyed as [OutputDirs] maps them) and the text it would
 * append to the dependency-file report (`depFile`).
 *
 * In mode `skip`, an answer replaces the KSP2 run. In modes `verify` and `shadow`, KSP2 runs anyway
 * and its outputs are compared byte-for-byte with the answer: `verify` fails the step on a mismatch
 * (for a census), `shadow` only records the outcome in the step's telemetry and keeps KSP2's
 * outputs (for a fleet-wide stage before `skip`). Without a resource, or with an unknown mode, KSP2
 * runs as before.
 */
class KspInertShortcut
private constructor(
    private val provider: String,
    private val shortcut: Class<*>,
    val mode: Mode,
) {

  enum class Mode {
    SKIP,
    VERIFY,
    SHADOW,
  }

  /** A shortcut's answer: [declined] or the [outputs] and [depFile] text. */
  class Answer(
      val declined: String?,
      val outputs: Map<String, ByteArray>,
      val depFile: String?,
  )

  /**
   * KSP2's output directories, and the one mapping between an answer's keys and their files, used
   * both to write an answer ([write]) and to read KSP2's outputs for comparison ([compare]): a file
   * under [classes] is keyed by its relative path; one under [kotlin] or [java] by that path after
   * [KOTLIN_SOURCES] or [JAVA_SOURCES].
   */
  class OutputDirs(val classes: File, val kotlin: File, val java: File) {

    /** The file an answer's [key] stands for; fails on a key that would leave its directory. */
    fun fileOf(key: String): File {
      val (dir, relative) =
          when {
            key.startsWith(KOTLIN_SOURCES) -> kotlin to key.removePrefix(KOTLIN_SOURCES)
            key.startsWith(JAVA_SOURCES) -> java to key.removePrefix(JAVA_SOURCES)
            else -> classes to key
          }
      val segments = relative.split('/')
      require(
          relative.isNotEmpty() &&
              !relative.startsWith("/") &&
              segments.none { it.isEmpty() || it == "." || it == ".." } &&
              !relative.contains('\\'),
      ) {
        "bad output key $key"
      }
      return File(dir, relative)
    }

    /** Every file KSP2 wrote, by key (the final-round placeholder left out). */
    fun keyedFiles(): Map<String, File> {
      val files = sortedMapOf<String, File>()
      for ((dir, prefix) in listOf(classes to "", kotlin to KOTLIN_SOURCES, java to JAVA_SOURCES)) {
        if (!dir.isDirectory) continue
        dir.walkTopDown()
            .filter { it.isFile }
            .forEach { file ->
              val relative = file.relativeTo(dir).invariantSeparatorsPath
              if (relative != FINAL_ROUND_PLACEHOLDER) {
                files[prefix + relative] = file
              }
            }
      }
      return files
    }
  }

  /**
   * Asks the shortcut; fails on an answer of the wrong shape (a non-`String` key, non-bytes value).
   */
  fun ask(sources: List<File>, options: Map<String, String>): Answer {
    val method = shortcut.getMethod("inertOutputs", List::class.java, Map::class.java)
    val result = method.invoke(null, sources.map { it.path }, options) as Map<*, *>
    val outputs = LinkedHashMap<String, ByteArray>()
    for ((key, value) in (result["outputs"] as Map<*, *>?).orEmpty()) {
      check(key is String && value is ByteArray) {
        "answer entry ${key?.javaClass?.name} -> ${value?.javaClass?.name}"
      }
      outputs[key] = value
    }
    return Answer(result["declined"] as String?, outputs, result["depFile"] as String?)
  }

  /**
   * Writes [answer] where KSP2 would have ([OutputDirs.fileOf]) and appends its report, all or
   * nothing: every key is checked, and no output may exist yet, before anything is written; on any
   * failure the files written so far are deleted and the report is restored, then the failure is
   * rethrown (the step then runs KSP2 on the directories as they were).
   */
  fun write(answer: Answer, dirs: OutputDirs, reportFile: File?) {
    val files = answer.outputs.map { (key, bytes) -> dirs.fileOf(key) to bytes }
    files
        .firstOrNull { (file, _) -> file.exists() }
        ?.let { (file, _) -> error("output exists: $file") }
    val reportBefore = reportFile?.takeIf { it.exists() }?.readBytes()
    val written = ArrayList<File>(files.size)
    try {
      for ((file, bytes) in files) {
        file.parentFile.mkdirs()
        written.add(file)
        file.writeBytes(bytes)
      }
      if (reportFile != null && answer.depFile != null) {
        reportFile.parentFile?.mkdirs()
        reportFile.appendText(answer.depFile, StandardCharsets.UTF_8)
      }
    } catch (e: Throwable) {
      written.forEach { it.delete() }
      if (reportFile != null) {
        if (reportBefore == null) reportFile.delete() else reportFile.writeBytes(reportBefore)
      }
      throw e
    }
  }

  /**
   * A comparison of KSP2's run with an answer: [mismatch] (null when they agree), [depFile] a note.
   */
  class Comparison(val mismatch: String?, val depFile: String)

  /**
   * KSP2's outputs ([OutputDirs.keyedFiles]) byte-for-byte against [answer]'s, and the classpath
   * files KSP2 appended to [reportFile] after [reportBefore] against the answer's. A
   * dependency-file entry only makes Buck rerun the action when that file changes, so fewer entries
   * than KSP2's are not a mismatch when the answer cannot depend on them; they are counted for the
   * census.
   */
  fun compare(
      answer: Answer,
      dirs: OutputDirs,
      reportFile: File?,
      reportBefore: String?,
  ): Comparison {
    val actual = dirs.keyedFiles().mapValues { (_, file) -> file.readBytes() }
    val problems = mutableListOf<String>()
    for (key in (actual.keys + answer.outputs.keys).toSortedSet()) {
      val a = actual[key]
      val b = answer.outputs[key]
      when {
        a == null -> problems.add("only the shortcut wrote $key")
        b == null -> problems.add("only KSP wrote $key (${a.size} bytes)")
        !a.contentEquals(b) -> problems.add("$key differs (${a.size} vs ${b.size} bytes)")
      }
    }
    var depFile = "no report"
    if (reportFile != null) {
      val after = if (reportFile.exists()) reportFile.readText(StandardCharsets.UTF_8) else null
      val appended = after?.removePrefix(reportBefore.orEmpty())
      val kspEntries = appended.orEmpty().lines().filter { it.isNotBlank() }
      val shortcutEntries = answer.depFile.orEmpty().lines().filter { it.isNotBlank() }
      if (!kspEntries.containsAll(shortcutEntries)) {
        problems.add("dependency file: the shortcut lists files KSP did not read")
      }
      depFile = "ksp ${kspEntries.size} shortcut ${shortcutEntries.size}"
    }
    return Comparison(if (problems.isEmpty()) null else problems.joinToString("; "), depFile)
  }

  override fun toString(): String = "$provider -> ${shortcut.name} ($mode)"

  /** What [lookup] found: no shortcut, a shortcut that failed, or a shortcut and its answer. */
  sealed class Lookup {
    object None : Lookup()

    class Failed(val error: Throwable) : Lookup()

    class Found(val shortcut: KspInertShortcut, val answer: Answer) : Lookup()
  }

  companion object {
    const val RESOURCE = "META-INF/kotlincd/ksp-inert-shortcut"
    const val FINAL_ROUND_PLACEHOLDER = "com/facebook/Dummy.java"
    const val MISMATCH = "KOTLINCD_KSP_INERT_SHORTCUT_MISMATCH"

    /** Key prefix of files in KSP2's Kotlin and Java output directories ([OutputDirs]). */
    const val KOTLIN_SOURCES = "ksp-kotlin-sources:"
    const val JAVA_SOURCES = "ksp-java-sources:"

    /**
     * The shortcut of the only provider in [providerNames], when its jar on [classLoader] declares
     * one; null when there are several providers or none declares a shortcut.
     */
    @JvmStatic
    fun find(classLoader: ClassLoader, providerNames: List<String>): KspInertShortcut? {
      val provider = providerNames.singleOrNull() ?: return null
      for (url in classLoader.getResources(RESOURCE)) {
        val properties = Properties()
        url.openStream().use { properties.load(it) }
        if (properties.getProperty("provider") != provider) {
          continue
        }
        val mode =
            when (properties.getProperty("mode")) {
              "skip" -> Mode.SKIP
              "verify" -> Mode.VERIFY
              "shadow" -> Mode.SHADOW
              else -> return null
            }
        val shortcut = Class.forName(properties.getProperty("shortcut"), true, classLoader)
        return KspInertShortcut(provider, shortcut, mode)
      }
      return null
    }

    /**
     * [find] and [ask], never throwing for a broken shortcut: any failure, including the errors a
     * broken class raises when it loads or runs (`ExceptionInInitializerError`,
     * `NoClassDefFoundError`, other `LinkageError`s), is returned as [Lookup.Failed], so the step
     * records it and runs KSP2. Only a [VirtualMachineError] (out of memory, stack overflow)
     * propagates.
     */
    @JvmStatic
    fun lookup(
        classLoader: ClassLoader,
        providerNames: List<String>,
        sources: List<File>,
        options: Map<String, String>,
    ): Lookup =
        guarded({ Lookup.Failed(it) }) {
          val shortcut = find(classLoader, providerNames) ?: return@guarded Lookup.None
          Lookup.Found(shortcut, shortcut.ask(sources, options))
        }

    /** [block], or [onFailure] of what it threw; a [VirtualMachineError] propagates. */
    @JvmStatic
    inline fun <T> guarded(onFailure: (Throwable) -> T, block: () -> T): T =
        try {
          block()
        } catch (e: VirtualMachineError) {
          throw e
        } catch (e: Throwable) {
          onFailure(e)
        }
  }
}
