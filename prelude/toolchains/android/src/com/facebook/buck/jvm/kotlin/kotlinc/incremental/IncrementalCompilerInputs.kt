/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.kotlinc.incremental

import com.facebook.buck.core.util.log.Logger
import java.io.File
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.charset.StandardCharsets
import java.nio.file.Files
import java.nio.file.InvalidPathException
import java.nio.file.Path
import java.nio.file.Paths
import java.nio.file.StandardCopyOption
import java.nio.file.attribute.UserDefinedFileAttributeView
import java.security.MessageDigest
import java.util.zip.ZipFile

/**
 * Inputs of an incremental kotlinc round that neither the Kotlin incremental compiler nor
 * `incremental_config.json` tracks: the content of the files that compiler options name, the order
 * of the classpath, and the Kotlin module mappings (the `.kotlin_module` files under `META-INF`) of
 * classpath jars.
 *
 * The incremental config records the settings kotlincd turns into compiler arguments, so that a
 * changed JVM target, friend path, compiler plugin or plugin option compiles in full. It records
 * paths, not content: a compiler plugin, an argument file (`@file`) or a file a plugin option
 * names, such as Compose's stability configuration, can change at the same path. kotlincd also
 * tells the incremental compiler that the classpath has not changed whenever the set of classpath
 * snapshots is the same, which hides a reordering that changes which of two duplicate classes the
 * compiler resolves. Classpath snapshots describe class files only, while the compiler finds
 * top-level declarations of a package through the module mappings, so a jar can gain or lose them
 * without any class changing.
 *
 * Each round records all of them in one file in the incremental state, as a pending record that
 * kotlincd promotes only when the action succeeds, so the committed record describes the last
 * successful round.
 */
object IncrementalCompilerInputs {
  const val RECORD_FILE: String = "incremental_inputs.txt"
  const val PENDING_RECORD_FILE: String = "incremental_inputs.pending.txt"

  private const val FILES = "files"
  private const val CLASSPATH_ORDER = "classpath"
  // EdenFS keeps the SHA-1 of every file of the checkout in this extended attribute.
  private const val SHA1_ATTRIBUTE = "sha1"
  private const val MODULE_MAPPINGS = "module"
  private const val NONE = "-"

  private val CLASSPATH_FLAGS = setOf("-classpath", "-cp")
  private const val PLUGIN_PREFIX = "-Xplugin="
  // K2's form: the plugin's classpath, then `=` and its options.
  private const val COMPILER_PLUGIN_PREFIX = "-Xcompiler-plugin="

  // Content-based output paths and configuration hashes in paths. A file's content is tracked by
  // its digest, so its hash segment must not count as a change.
  private val HASH_SEGMENT = Regex("(?<=/)[0-9a-f]{16}(?=/)")

  // The buck-out isolation directory (`v2`, or a test's own), so that a record written in another
  // isolation directory or on another machine, such as state a cache hit brought in, still
  // compares.
  private val ISOLATION_DIR = Regex("(^|/)buck-out/[^/]+/")

  private val SHA1 = Regex("[0-9a-f]{40}")

  // Friend paths are classpath entries, whose content the classpath snapshots track.
  private const val FRIEND_PATHS_PREFIX = "-Xfriend-paths="
  // Kotlin Multiplatform names each fragment's sources in an option; the sources are tracked.
  private const val FRAGMENT_SOURCES_PREFIX = "-Xfragment-sources="
  private const val ARGUMENT_FILE_PREFIX = "@"

  // The digest of a file that exists but cannot be read.
  private const val UNREADABLE = "?"

  /**
   * Which untracked inputs differ from the last successful round's. [recordUnreadable] is set when
   * the last successful round's record could not be read, so that everything counts as changed.
   */
  data class Changes(
      val filesChanged: Boolean,
      val classpathOrderChanged: Boolean,
      val moduleMappingsChanged: Boolean = false,
      /** How many classpath jars this round read module mappings from. */
      val moduleMappingJarsRead: Int = 0,
      val recordUnreadable: Boolean = false,
  )

  /**
   * [mode], compiling in full when a compiler plugin or a file a compiler option names changed
   * since the last successful round, and letting the incremental compiler compute classpath changes
   * when the classpath order changed. Returns [mode] itself, touching nothing, when kotlincd does
   * not track these inputs.
   *
   * @param sources the round's sources, which the incremental compiler tracks itself
   * @param onCompared receives the changes and the milliseconds spent finding them
   */
  @JvmStatic
  fun apply(
      mode: KotlincMode,
      compilerArgs: List<String>,
      sources: Collection<Path>,
      onCompared: (Changes, Long) -> Unit,
  ): KotlincMode {
    if (mode !is KotlincMode.Incremental) return mode
    val inputTracking = mode.inputTracking ?: return mode
    val start = System.nanoTime()
    val incrementalStateDir = checkNotNull(mode.kotlicWorkingDir.path.parent)
    val changes =
        try {
          recordAndCompare(
              incrementalStateDir,
              mode.rootProjectDir.path,
              compilerArgs,
              inputTracking.inputDigests,
              inputTracking.optionValues,
              sources,
          )
        } catch (e: IOException) {
          LOG.warn(e, "Could not record the incremental compiler inputs; compiling in full")
          return mode.withRebuildReason(RebuildReason.INCREMENTAL_INPUTS_UNREADABLE)
        } catch (e: Exception) {
          // Any other failure, such as a path that cannot be parsed, also compiles in full. The
          // records go too, so that no later round compares with inputs older than its outputs.
          LOG.warn(e, "Could not record the incremental compiler inputs; compiling in full")
          dropRecords(incrementalStateDir)
          return mode.withRebuildReason(RebuildReason.INCREMENTAL_INPUTS_UNREADABLE)
        }
    onCompared(changes, (System.nanoTime() - start) / 1_000_000)
    var result = mode
    if (changes.recordUnreadable) {
      result = result.withRebuildReason(RebuildReason.INCREMENTAL_INPUTS_UNREADABLE)
    } else if (changes.filesChanged) {
      result = result.withRebuildReason(RebuildReason.COMPILER_INPUT_FILES_CHANGED)
    }
    if (changes.moduleMappingsChanged) {
      result = result.withRebuildReason(RebuildReason.KOTLIN_MODULE_MAPPINGS_CHANGED)
    }
    val classpathChanges = result.classpathChanges
    if (changes.classpathOrderChanged && classpathChanges is ClasspathChanges.NoChanges) {
      result =
          result.copy(
              classpathChanges =
                  ClasspathChanges.ToBeComputedByIncrementalCompiler(
                      classpathChanges.classpathSnapshotFiles,
                  ),
          )
    }
    return result
  }

  /**
   * Records the digests of this round's compiler plugins and of the files its option values name,
   * its classpath order and its module mappings as pending in [incrementalStateDir], and compares
   * them with the last successful round's. Without a committed record, or with one that cannot be
   * read, everything counts as changed, and the round still records what it compiles with. A plugin
   * or a named file that cannot be read counts as changed. When this round's record cannot be made,
   * the committed record is deleted too, so that no later round compares with a record older than
   * its outputs.
   *
   * Module mappings are recorded for every classpath entry, with the entry's input digest (or, for
   * a jar without one, its [contentDigest] when the file system keeps one), and read only from the
   * jars that the record does not hold with the same path and digest. Directory entries are read
   * every round.
   *
   * @param arguments the compiler arguments
   * @param inputDigests the action's input digests by project-relative path, which identify the
   *   content of compiler plugins, named files and classpath jars
   * @param optionValues the target's own compiler arguments and plugin option values, which may
   *   name files (see [fileValues])
   * @param sources the round's sources, absolute or relative to [rootProjectDir]: an option value
   *   that names one is left to the incremental compiler
   */
  @JvmStatic
  @Throws(IOException::class)
  fun recordAndCompare(
      incrementalStateDir: Path,
      rootProjectDir: Path,
      arguments: List<String>,
      inputDigests: Map<Path, String>,
      optionValues: List<String>,
      sources: Collection<Path> = emptyList(),
  ): Changes {
    val root = rootProjectDir.toString() + File.separator
    try {
      // First, so that a read failing below leaves no earlier round's record to promote.
      Files.deleteIfExists(incrementalStateDir.resolve(PENDING_RECORD_FILE))
      var recordUnreadable = false
      val previousLines =
          try {
            readRecord(incrementalStateDir)
          } catch (e: IOException) {
            recordUnreadable = true
            null
          }
      val previous = previousLines?.let(::recordValues)
      val previousMappings = previousLines.orEmpty().mapNotNull { ModuleMappingEntry.parse(it) }
      val recordedMappings =
          previousMappings.associateBy({ it.path to it.inputDigest }, { it.mappings })
      var jarsRead = 0
      val currentMappings =
          classpathEntries(arguments).map { entry ->
            val path = entry.removePrefix(root)
            val inputDigest =
                inputDigests[Paths.get(path)]
                    ?: contentDigest(rootProjectDir.resolve(path), hashContent = false)
                    ?: NONE
            val recorded = if (inputDigest == NONE) null else recordedMappings[path to inputDigest]
            val mappings =
                recorded
                    ?: run {
                      jarsRead++
                      moduleMappingsDigest(rootProjectDir.resolve(path))
                    }
            ModuleMappingEntry(path, inputDigest, mappings)
          }
      val files = fileDigests(arguments, optionValues, sources, root, rootProjectDir, inputDigests)
      val filesDigest = digest(files.map { (path, digest) -> "$path#$digest" })
      val classpathOrderDigest = digest(classpathOrder(arguments, root))
      writePending(
          incrementalStateDir,
          listOf("$FILES $filesDigest", "$CLASSPATH_ORDER $classpathOrderDigest") +
              currentMappings.map { it.format() },
      )
      return Changes(
          filesChanged =
              previous?.get(FILES) != filesDigest ||
                  files.any { (_, digest) -> digest == UNREADABLE },
          classpathOrderChanged = previous?.get(CLASSPATH_ORDER) != classpathOrderDigest,
          moduleMappingsChanged =
              previousLines == null ||
                  mappingIdentities(previousMappings, root) !=
                      mappingIdentities(currentMappings, root),
          moduleMappingJarsRead = jarsRead,
          recordUnreadable = recordUnreadable,
      )
    } catch (e: IOException) {
      Files.deleteIfExists(incrementalStateDir.resolve(RECORD_FILE))
      throw e
    }
  }

  /**
   * The digests of the files whose content the compiler reads besides sources and the classpath, by
   * portable path, sorted, with an entry for every file even where two portable paths are equal:
   * every compiler plugin, by its input digest or, without one (a symlink Buck2 makes to a prebuilt
   * or toolchain jar in the source tree), by the digest of its content; and every file an option
   * value names (see [fileDigest]) that is not one of the round's [sources].
   */
  private fun fileDigests(
      arguments: List<String>,
      optionValues: List<String>,
      sources: Collection<Path>,
      root: String,
      rootProjectDir: Path,
      inputDigests: Map<Path, String>,
  ): List<Pair<String, String>> {
    val digests = ArrayList<Pair<String, String>>()
    for (plugin in pluginJars(arguments)) {
      val relativePath = relative(plugin, root)
      digests +=
          portablePath(plugin, root) to
              (inputDigests[relativePath]
                  ?: contentDigest(rootProjectDir.resolve(relativePath), hashContent = true)
                  ?: UNREADABLE)
    }
    val sourceFiles = sources.mapTo(HashSet()) { rootProjectDir.resolve(it).normalize() }
    for (value in optionValues.flatMap(::fileValues)) {
      if (namesSource(value, root, rootProjectDir, sourceFiles)) continue
      fileDigest(value, root, rootProjectDir, inputDigests)?.let {
        digests += portablePath(value, root) to it
      }
    }
    return digests.sortedWith(compareBy({ it.first }, { it.second }))
  }

  /** Whether [value] names one of the round's [sources], which the incremental compiler tracks. */
  private fun namesSource(
      value: String,
      root: String,
      rootProjectDir: Path,
      sources: Set<Path>,
  ): Boolean =
      sources.isNotEmpty() &&
          try {
            rootProjectDir.resolve(relative(value, root)).normalize() in sources
          } catch (e: InvalidPathException) {
            false
          }

  /**
   * The parts of an option value that may name a file: the value itself, without the `@` of an
   * argument file, and, after its first `=`, the rest and its comma- or path-separated elements.
   * Friend paths and multiplatform fragment sources are left out: they are classpath entries and
   * sources.
   */
  private fun fileValues(value: String): Set<String> {
    if (value.startsWith(FRIEND_PATHS_PREFIX) || value.startsWith(FRAGMENT_SOURCES_PREFIX)) {
      return emptySet()
    }
    val candidates = LinkedHashSet<String>()
    candidates += value.removePrefix(ARGUMENT_FILE_PREFIX)
    val rest = value.substringAfter('=', "")
    if (rest.isNotEmpty()) {
      candidates += rest
      rest.split(',', File.pathSeparatorChar).filterTo(candidates) { it.isNotEmpty() }
    }
    return candidates.filterTo(LinkedHashSet()) { it.isNotEmpty() && !it.startsWith("-") }
  }

  /**
   * The digest of the file a value names, or null when it names no file that kotlincd tracks this
   * way:
   * - an action input, whatever the form of the path: its input digest;
   * - a file named through a symlink whose resolved path is an action input: that input's digest;
   * - a readable file outside `buck-out` (the source tree), named by a path with a separator: its
   *   [contentDigest];
   * - any other file under `buck-out` is an output or a temporary file of the action.
   *
   * A file that exists but cannot be read gives [UNREADABLE].
   */
  private fun fileDigest(
      value: String,
      root: String,
      rootProjectDir: Path,
      inputDigests: Map<Path, String>,
  ): String? {
    val relativePath =
        try {
          relative(value, root)
        } catch (e: InvalidPathException) {
          return null
        }
    inputDigests[relativePath]?.let {
      return it
    }
    // A value without a separator counts only as an action input: tokens such as class names are
    // not looked up on disk.
    if ('/' !in value) return null
    val file = rootProjectDir.resolve(relativePath)
    if (!Files.isRegularFile(file)) return null
    val real =
        try {
          file.toRealPath()
        } catch (e: IOException) {
          return UNREADABLE
        }
    // Buck2 leaves symlinks out of the action metadata and lists the artifact they resolve to, so a
    // declared input named through a symlink is found by its resolved path.
    val resolved = projectRelative(real, rootProjectDir)
    resolved
        ?.let { inputDigests[it] }
        ?.let {
          return it
        }
    if (resolved != null && resolved.startsWith("buck-out")) return null
    return contentDigest(real, hashContent = true) ?: UNREADABLE
  }

  /** [path] relative to [rootProjectDir] or to the directory it resolves to, or null outside it. */
  private fun projectRelative(path: Path, rootProjectDir: Path): Path? {
    if (path.startsWith(rootProjectDir)) return rootProjectDir.relativize(path)
    val realRoot =
        try {
          rootProjectDir.toRealPath()
        } catch (e: IOException) {
          return null
        }
    return if (path.startsWith(realRoot)) realRoot.relativize(path) else null
  }

  /** The `<key> <value>` lines of a record, by key. */
  private fun recordValues(lines: List<String>): Map<String, String> =
      lines
          .mapNotNull { line -> line.split(' ', limit = 2).takeIf { it.size == 2 } }
          .associate { (key, value) -> key to value }

  /**
   * The SHA-1 of the content of the file [file] resolves to, or null when the file cannot be read.
   * EdenFS keeps the SHA-1 of every file of the checkout, so a file there is not read; elsewhere
   * the content is read only when [hashContent] is set, and the result is null otherwise. The value
   * is the same either way, so records compare across machines.
   */
  private fun contentDigest(file: Path, hashContent: Boolean): String? =
      try {
        val real = file.toRealPath()
        when {
          !Files.isRegularFile(real) || !Files.isReadable(real) -> null
          else -> storedSha1(real) ?: if (hashContent) sha1(real) else null
        }
      } catch (e: IOException) {
        null
      }

  private fun storedSha1(file: Path): String? =
      try {
        val view =
            Files.getFileAttributeView(file, UserDefinedFileAttributeView::class.java)
                ?: return null
        val buffer = ByteBuffer.allocate(view.size(SHA1_ATTRIBUTE))
        view.read(SHA1_ATTRIBUTE, buffer)
        buffer.flip()
        StandardCharsets.US_ASCII.decode(buffer)
            .toString()
            .trim()
            .takeIf { SHA1.matches(it) }
            ?.let { "sha1:$it" }
      } catch (e: IOException) {
        null
      } catch (e: UnsupportedOperationException) {
        null
      }

  private fun sha1(file: Path): String {
    val digest = MessageDigest.getInstance("SHA-1").digest(Files.readAllBytes(file))
    return "sha1:" + digest.joinToString("") { "%02x".format(it) }
  }

  /** One classpath entry's module mappings, as recorded: its path, input digest and mappings. */
  private data class ModuleMappingEntry(
      val path: String,
      val inputDigest: String,
      val mappings: String,
  ) {
    fun format(): String = "$MODULE_MAPPINGS $path\t$inputDigest\t$mappings"

    companion object {
      fun parse(line: String): ModuleMappingEntry? {
        if (!line.startsWith("$MODULE_MAPPINGS ")) return null
        val fields = line.removePrefix("$MODULE_MAPPINGS ").split('\t')
        return if (fields.size == 3) ModuleMappingEntry(fields[0], fields[1], fields[2]) else null
      }
    }
  }

  /**
   * The entries that carry module mappings, each as its path without hash segments and a digest of
   * its mappings, counted. An entry that gains, loses or changes mappings, or carries mappings and
   * enters or leaves the classpath, changes it; a new content hash in a path alone does not.
   */
  private fun mappingIdentities(
      entries: List<ModuleMappingEntry>,
      root: String,
  ): Map<Pair<String, String>, Int> =
      entries
          .filter { it.mappings != NONE }
          .groupingBy { portablePath(it.path, root) to it.mappings }
          .eachCount()

  /** A digest of the `.kotlin_module` entries under `META-INF` in [jar], or [NONE] without any. */
  private fun moduleMappingsDigest(jar: Path): String {
    if (Files.isDirectory(jar)) return directoryModuleMappingsDigest(jar)
    if (!Files.isRegularFile(jar)) return NONE
    ZipFile(jar.toFile()).use { zip ->
      val modules =
          zip.entries()
              .asSequence()
              .filter { it.name.startsWith("META-INF/") && it.name.endsWith(".kotlin_module") }
              .sortedBy { it.name }
              .toList()
      if (modules.isEmpty()) return NONE
      val digest = MessageDigest.getInstance("SHA-256")
      for (module in modules) {
        digest.update(module.name.toByteArray(StandardCharsets.UTF_8))
        digest.update(0)
        zip.getInputStream(module).use { digest.update(it.readBytes()) }
      }
      return digest.digest().joinToString("") { "%02x".format(it) }
    }
  }

  /** The same digest for a classpath directory: its `.kotlin_module` files under `META-INF`. */
  private fun directoryModuleMappingsDigest(directory: Path): String {
    val metaInf = directory.resolve("META-INF")
    if (!Files.isDirectory(metaInf)) return NONE
    val modules: List<Path> =
        Files.list(metaInf).use { files ->
          files
              .iterator()
              .asSequence()
              .filter {
                it.fileName.toString().endsWith(".kotlin_module") && Files.isRegularFile(it)
              }
              .sortedBy { it.fileName.toString() }
              .toList()
        }
    if (modules.isEmpty()) return NONE
    val digest = MessageDigest.getInstance("SHA-256")
    for (module in modules) {
      digest.update("META-INF/${module.fileName}".toByteArray(StandardCharsets.UTF_8))
      digest.update(0)
      digest.update(Files.readAllBytes(module))
    }
    return digest.digest().joinToString("") { "%02x".format(it) }
  }

  /**
   * Makes the pending record the committed one. Call only once the action succeeded, and only for
   * an action that tracks these inputs. If that fails, the record is dropped, so that the next
   * round compiles in full.
   */
  @JvmStatic
  fun promotePending(incrementalStateDir: Path) {
    val pending = incrementalStateDir.resolve(PENDING_RECORD_FILE)
    val committed = incrementalStateDir.resolve(RECORD_FILE)
    try {
      if (Files.exists(pending)) {
        Files.move(
            pending,
            committed,
            StandardCopyOption.REPLACE_EXISTING,
            StandardCopyOption.ATOMIC_MOVE,
        )
      }
    } catch (e: IOException) {
      LOG.warn(e, "Could not promote %s; the next round compiles in full", pending)
      try {
        Files.deleteIfExists(committed)
      } catch (deleteFailure: IOException) {
        LOG.warn(deleteFailure, "Could not delete %s", committed)
      }
    }
  }

  /** Deletes the pending and the committed record, so that the next round compiles in full. */
  private fun dropRecords(incrementalStateDir: Path) {
    for (record in listOf(PENDING_RECORD_FILE, RECORD_FILE)) {
      try {
        Files.deleteIfExists(incrementalStateDir.resolve(record))
      } catch (e: IOException) {
        LOG.warn(e, "Could not delete %s", incrementalStateDir.resolve(record))
      }
    }
  }

  /**
   * The classpath entries in the order the compiler searches them, duplicates included, each with
   * its whole path: two entries that differ only in a hash segment are different jars.
   */
  fun classpathOrder(arguments: List<String>, root: String): List<String> =
      classpathEntries(arguments).map {
        ISOLATION_DIR.replace(it.removePrefix(root), "$1buck-out/_/")
      }

  /** The entries of every classpath flag's value, in order. */
  private fun classpathEntries(arguments: List<String>): List<String> {
    val entries = ArrayList<String>()
    var index = 0
    while (index < arguments.size - 1) {
      if (arguments[index] in CLASSPATH_FLAGS) {
        arguments[index + 1].split(File.pathSeparatorChar).filterTo(entries) { it.isNotEmpty() }
        index += 2
      } else {
        index++
      }
    }
    return entries
  }

  private fun pluginJars(arguments: List<String>): List<String> =
      arguments
          .flatMap {
            when {
              it.startsWith(PLUGIN_PREFIX) -> it.removePrefix(PLUGIN_PREFIX).split(',')
              it.startsWith(COMPILER_PLUGIN_PREFIX) ->
                  it.removePrefix(COMPILER_PLUGIN_PREFIX).substringBefore('=').split(',')
              else -> emptyList()
            }
          }
          .filter { it.isNotEmpty() }

  private fun relative(path: String, root: String): Path = Paths.get(path.removePrefix(root))

  /** [path] relative to [root], without the isolation directory and hash segments. */
  private fun portablePath(path: String, root: String): String =
      HASH_SEGMENT.replace(ISOLATION_DIR.replace(path.removePrefix(root), "$1buck-out/_/"), "_")

  private fun digest(values: List<String>): String {
    val digest = MessageDigest.getInstance("SHA-256")
    for (value in values) {
      digest.update(value.toByteArray(StandardCharsets.UTF_8))
      digest.update(0)
    }
    return digest.digest().joinToString("") { "%02x".format(it) }
  }

  /** The lines of the committed record, or null when there is none. */
  private fun readRecord(incrementalStateDir: Path): List<String>? {
    val committed = incrementalStateDir.resolve(RECORD_FILE)
    if (!Files.exists(committed)) return null
    return Files.readAllLines(committed, StandardCharsets.UTF_8)
  }

  private fun writePending(incrementalStateDir: Path, lines: List<String>) {
    Files.createDirectories(incrementalStateDir)
    // Complete or absent, so that a round stopped while writing leaves nothing to promote.
    val temporary = Files.createTempFile(incrementalStateDir, PENDING_RECORD_FILE, ".tmp")
    try {
      Files.write(temporary, lines, StandardCharsets.UTF_8)
      Files.move(
          temporary,
          incrementalStateDir.resolve(PENDING_RECORD_FILE),
          StandardCopyOption.ATOMIC_MOVE,
      )
    } finally {
      Files.deleteIfExists(temporary)
    }
  }

  private val LOG: Logger = Logger.get(IncrementalCompilerInputs::class.java)
}
