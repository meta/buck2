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

import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.ClasspathChanges
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.IncrementalCompilerInputs
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.IncrementalInputTracking
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason
import java.io.IOException
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.nio.file.attribute.FileTime
import java.nio.file.attribute.PosixFilePermissions
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeFalse
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class IncrementalCompilerInputsTest {

  @get:Rule val temporaryFolder = TemporaryFolder()

  private lateinit var root: Path
  private lateinit var stateDir: Path

  @Before
  fun setUp() {
    root = temporaryFolder.root.toPath()
    stateDir = temporaryFolder.newFolder("incremental_state").toPath()
  }

  @Test
  fun `the first round counts as changed, an unchanged round does not`() {
    val first = record()
    assertTrue(first.filesChanged)
    assertTrue(first.classpathOrderChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    val second = record()
    assertFalse(second.filesChanged)
    assertFalse(second.classpathOrderChanged)
  }

  @Test
  fun `a new plugin digest changes the files, a new dependency path only the order`() {
    record()
    IncrementalCompilerInputs.promotePending(stateDir)

    val newDependency = record(dependencyHash = "fedcba9876543210")
    assertFalse(newDependency.filesChanged)
    assertTrue(newDependency.classpathOrderChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(record(dependencyHash = "fedcba9876543210", pluginDigest = "b").filesChanged)
  }

  @Test
  fun `option values that name no file are left to the incremental config`() {
    record(optionValues = listOf("-P", "plugin:org.jetbrains.kotlin.allopen:annotation=p.Open"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(
        record(
            optionValues = listOf("-P", "plugin:org.jetbrains.kotlin.allopen:annotation=p.Other"),
        )
            .filesChanged,
    )
  }

  @Test
  fun `an unreadable plugin without an input digest always counts as changed`() {
    record(pluginDigest = null)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(record(pluginDigest = null).filesChanged)
  }

  @Test
  fun `a plugin without an input digest is compared by its content`() {
    val plugin = root.resolve("plugin.jar")
    Files.writeString(plugin, "first")
    record(pluginDigest = null)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(record(pluginDigest = null).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    // Rewritten with the same content.
    Files.writeString(plugin, "first")
    Files.setLastModifiedTime(plugin, FileTime.fromMillis(1_000))
    assertFalse(record(pluginDigest = null).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    // Other content of the same size, with the same modification time.
    Files.writeString(plugin, "other")
    Files.setLastModifiedTime(plugin, FileTime.fromMillis(1_000))
    assertTrue(record(pluginDigest = null).filesChanged)
  }

  @Test
  fun `a plugin without an input digest is identified by the SHA-1 of its content`() {
    // The value EdenFS keeps for a file of the checkout, so that a record from another machine
    // compares equal.
    val plugin = root.resolve("plugin.jar")
    Files.writeString(plugin, "first")
    val arguments = listOf("-Xplugin=$root/plugin.jar", "$root/src/A.kt")
    val canonical =
        IncrementalCompilerInputs.recordAndCompare(
            stateDir,
            root,
            arguments,
            emptyMap(),
            emptyList(),
        )
    IncrementalCompilerInputs.promotePending(stateDir)
    val withDigest =
        IncrementalCompilerInputs.recordAndCompare(
            stateDir,
            root,
            arguments,
            mapOf(
                Paths.get("plugin.jar") to
                    "sha1:" +
                        java.security.MessageDigest.getInstance("SHA-1")
                            .digest("first".toByteArray())
                            .joinToString("") { "%02x".format(it) },
            ),
            emptyList(),
        )

    assertTrue(canonical.filesChanged)
    assertFalse(withDigest.filesChanged)
  }

  @Test
  fun `a plugin without an input digest that cannot be read counts as changed`() {
    val plugin = root.resolve("plugin.jar")
    Files.writeString(plugin, "first")
    record(pluginDigest = null)
    IncrementalCompilerInputs.promotePending(stateDir)

    Files.setPosixFilePermissions(plugin, emptySet())
    try {
      assumeFalse("Running as a user that reads any file", Files.isReadable(plugin))
      assertTrue(record(pluginDigest = null).filesChanged)
    } finally {
      Files.setPosixFilePermissions(plugin, PosixFilePermissions.fromString("rw-r--r--"))
    }
  }

  @Test
  fun `a symlinked plugin is compared by the content of the file it resolves to`() {
    val first = root.resolve("plugins/first.jar")
    val second = root.resolve("plugins/second.jar")
    Files.createDirectories(first.parent)
    Files.writeString(first, "plugin-a")
    Files.writeString(second, "plugin-b")
    Files.setLastModifiedTime(second, Files.getLastModifiedTime(first))
    val link = root.resolve("plugin.jar")
    Files.createSymbolicLink(link, first)
    record(pluginDigest = null)
    IncrementalCompilerInputs.promotePending(stateDir)

    // Same size and time, another file.
    Files.delete(link)
    Files.createSymbolicLink(link, second)

    assertTrue(record(pluginDigest = null).filesChanged)
  }

  @Test
  fun `a record from another buck-out isolation directory compares equal`() {
    fun recordIn(isolation: String) =
        IncrementalCompilerInputs.recordAndCompare(
            stateDir,
            root,
            listOf(
                "-classpath",
                "$root/buck-out/$isolation/art/lib/__lib__/lib.jar",
                "-Xplugin=$root/buck-out/$isolation/art/p/__p__/p.jar",
            ),
            mapOf(Paths.get("buck-out/$isolation/art/p/__p__/p.jar") to "a"),
            emptyList(),
        )
    recordIn("v2")
    IncrementalCompilerInputs.promotePending(stateDir)

    val changes = recordIn(".buck_e2e0123")

    assertFalse(changes.filesChanged)
    assertFalse(changes.classpathOrderChanged)
  }

  @Test
  fun `a reorder within a later classpath flag changes the classpath order`() {
    fun recordWith(second: List<String>) =
        IncrementalCompilerInputs.recordAndCompare(
            stateDir,
            root,
            listOf(
                "-classpath",
                "$root/first.jar",
                "-cp",
                second.joinToString(java.io.File.pathSeparator) { "$root/$it" },
            ),
            emptyMap(),
            emptyList(),
        )
    recordWith(listOf("a.jar", "b.jar"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(recordWith(listOf("a.jar", "b.jar")).classpathOrderChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(recordWith(listOf("b.jar", "a.jar")).classpathOrderChanged)
  }

  @Test
  fun `a reordered classpath changes only the classpath order, duplicates included`() {
    record(classpath = listOf("first.jar", "second.jar"))
    IncrementalCompilerInputs.promotePending(stateDir)

    val reordered = record(classpath = listOf("second.jar", "first.jar"))
    assertTrue(reordered.classpathOrderChanged)
    assertFalse(reordered.filesChanged)

    assertTrue(
        record(classpath = listOf("first.jar", "second.jar", "first.jar")).classpathOrderChanged,
    )
  }

  @Test
  fun `jars with the same name under different hash segments keep their order`() {
    val first = "$root/buck-out/v2/art/__lib__/0123456789abcdef/same.jar"
    val second = "$root/buck-out/v2/art/__lib__/fedcba9876543210/same.jar"
    val order =
        IncrementalCompilerInputs.classpathOrder(
            listOf("-cp", listOf(first, second).joinToString(java.io.File.pathSeparator)),
            "$root/",
        )
    val reversed =
        IncrementalCompilerInputs.classpathOrder(
            listOf("-cp", listOf(second, first).joinToString(java.io.File.pathSeparator)),
            "$root/",
        )

    assertNotEquals(order, reversed)
  }

  @Test
  fun `a failed round leaves the last successful round's record`() {
    record(pluginDigest = "a")
    IncrementalCompilerInputs.promotePending(stateDir)

    // A round with another plugin that fails: nothing promotes its pending record.
    assertTrue(record(pluginDigest = "b").filesChanged)

    assertFalse(record(pluginDigest = "a").filesChanged)
  }

  @Test
  fun `a stale pending record is replaced before it can be promoted`() {
    record()
    IncrementalCompilerInputs.promotePending(stateDir)
    val stalePending = stateDir.resolve(IncrementalCompilerInputs.PENDING_RECORD_FILE)
    Files.writeString(stalePending, "stale")

    record()

    assertEquals(
        Files.readString(stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE)),
        Files.readString(stalePending),
    )
  }

  @Test
  fun `a round whose record cannot be read compiles in full and records its own inputs`() {
    record(pluginDigest = "a")
    IncrementalCompilerInputs.promotePending(stateDir)
    val pending = stateDir.resolve(IncrementalCompilerInputs.PENDING_RECORD_FILE)
    Files.writeString(pending, "stale")
    val committed = stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE)
    Files.setPosixFilePermissions(committed, emptySet())
    assumeFalse("Running as a user that reads any file", Files.isReadable(committed))

    val changes = record(pluginDigest = "b")
    assertTrue(changes.recordUnreadable)
    assertTrue(changes.filesChanged)
    assertNotEquals("stale", Files.readString(pending))
    IncrementalCompilerInputs.promotePending(stateDir)

    // The record now holds the inputs the full round compiled with, not the older ones.
    assertTrue(record(pluginDigest = "a").filesChanged)
    assertFalse(record(pluginDigest = "b").filesChanged)
  }

  @Test
  fun `a round whose record cannot be made deletes the committed record`() {
    record()
    IncrementalCompilerInputs.promotePending(stateDir)
    // A non-empty directory where the pending record goes: it can be neither replaced nor written.
    Files.createDirectories(
        stateDir.resolve(IncrementalCompilerInputs.PENDING_RECORD_FILE).resolve("blocker"),
    )

    assertThrows(IOException::class.java) { record(pluginDigest = "b") }

    assertFalse(Files.exists(stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE)))
  }

  @Test
  fun `a file an option names whose content changes at the same path counts as changed`() {
    val config = root.resolve("compose/stability.conf")
    Files.createDirectories(config.parent)
    Files.writeString(config, "a.Stable")
    val option = listOf("-P", "plugin:x:stabilityConfigurationPath=$config")
    record(optionValues = option)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(record(optionValues = option).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    // Same size and path, other content.
    Files.writeString(config, "b.Stable")
    assertTrue(record(optionValues = option).filesChanged)
  }

  @Test
  fun `an argument file is compared by its content`() {
    val arguments = root.resolve("args.txt")
    Files.writeString(arguments, "-jvm-target 1.8")
    val option = listOf("@$arguments")
    record(optionValues = option)
    IncrementalCompilerInputs.promotePending(stateDir)
    assertFalse(record(optionValues = option).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    Files.writeString(arguments, "-jvm-target 11.")
    assertTrue(record(optionValues = option).filesChanged)
  }

  @Test
  fun `an option naming an action input is compared by the input's digest`() {
    val input = Paths.get("buck-out/v2/gen/config.txt")
    Files.createDirectories(root.resolve(input).parent)
    Files.writeString(root.resolve(input), "content")
    val option = listOf("-P", "plugin:x:config=$root/$input")
    record(optionValues = option, extraDigests = mapOf(input to "first"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(record(optionValues = option, extraDigests = mapOf(input to "first")).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(record(optionValues = option, extraDigests = mapOf(input to "second")).filesChanged)
  }

  @Test
  fun `a declared input named through a symlink is compared by the digest of the file it resolves to`() {
    // Buck2 leaves symlinks out of the action metadata and lists the artifact they resolve to.
    val target = Paths.get("buck-out/v2/gen/fbsource/0123456789abcdef/stability/stability.conf")
    Files.createDirectories(root.resolve(target).parent)
    Files.writeString(root.resolve(target), "a.Stable")
    val link = root.resolve("buck-out/v2/gen/fbsource/fedcba9876543210/render/stability.conf")
    Files.createDirectories(link.parent)
    Files.createSymbolicLink(link, root.resolve(target))
    val option = listOf("-P", "plugin:x:stabilityConfigurationPath=$link")
    record(optionValues = option, extraDigests = mapOf(target to "first"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(record(optionValues = option, extraDigests = mapOf(target to "first")).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    // The option names the same symlink; the file it resolves to has new content.
    assertTrue(record(optionValues = option, extraDigests = mapOf(target to "second")).filesChanged)
  }

  @Test
  fun `a record that cannot be promoted leaves the next round compiling in full`() {
    record()
    IncrementalCompilerInputs.promotePending(stateDir)
    record(pluginDigest = "b")
    // A non-empty directory where the committed record goes: it can be neither replaced nor
    // deleted.
    val committed = stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE)
    Files.delete(committed)
    Files.createDirectories(committed.resolve("blocker"))

    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(record(pluginDigest = "b").recordUnreadable)
  }

  @Test
  fun `an option naming an action input by a bare relative path is compared by its digest`() {
    val input = Paths.get("config.conf")
    val option = listOf("-P", "plugin:test:config=config.conf")
    record(optionValues = option, extraDigests = mapOf(input to "first"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(record(optionValues = option, extraDigests = mapOf(input to "first")).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(record(optionValues = option, extraDigests = mapOf(input to "second")).filesChanged)
  }

  @Test
  fun `each plugin option value that names a file is compared`() {
    // A compiler plugin option value is the value alone, here an absolute path.
    val config = Paths.get("buck-out/v2/gen/stability.txt")
    val jar = Paths.get("buck-out/v2/art/dep/__dep__/plugin-input.jar")
    val values = listOf("$root/$config", "$root/$jar")
    record(optionValues = values, extraDigests = mapOf(config to "a", jar to "a"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(
        record(optionValues = values, extraDigests = mapOf(config to "b", jar to "a")).filesChanged,
    )
    IncrementalCompilerInputs.promotePending(stateDir)
    assertTrue(
        record(optionValues = values, extraDigests = mapOf(config to "b", jar to "b")).filesChanged,
    )
  }

  @Test
  fun `multiplatform fragment sources are left to the incremental compiler`() {
    // A rule in a cell at the project root names each fragment source by its project-relative
    // path, the path of the source's action input digest.
    val source = Paths.get("lib/src/commonMain/kotlin/Common.kt")
    val option = listOf("-Xfragment-sources=commonMain:$source")
    record(optionValues = option, extraDigests = mapOf(source to "first"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(
        record(optionValues = option, extraDigests = mapOf(source to "second")).filesChanged,
    )
  }

  @Test
  fun `an option naming one of the round's sources is left to the incremental compiler`() {
    val source = Paths.get("lib/src/commonMain/kotlin/Common.kt")
    val option = listOf("-Xcommon-sources=$source")
    val sources = listOf(root.resolve(source))
    record(optionValues = option, extraDigests = mapOf(source to "first"), sources = sources)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(
        record(optionValues = option, extraDigests = mapOf(source to "second"), sources = sources)
            .filesChanged,
    )
    // The same file when it is not a source of the round.
    assertTrue(record(optionValues = option, extraDigests = mapOf(source to "second")).filesChanged)
  }

  @Test
  fun `a plugin passed in K2's form is compared by its digest`() {
    val plugin = Paths.get("k2plugin.jar")
    val options = listOf("-Xcompiler-plugin=$root/$plugin=option=value")
    record(optionValues = options, extraDigests = mapOf(plugin to "first"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(
        record(optionValues = options, extraDigests = mapOf(plugin to "first")).filesChanged,
    )
    assertTrue(
        record(optionValues = options, extraDigests = mapOf(plugin to "second")).filesChanged,
    )
  }

  @Test
  fun `two files whose paths differ only in a hash segment are both compared`() {
    val first = Paths.get("buck-out/v2/gen/fixture/0123456789abcdef/stability.conf")
    val second = Paths.get("buck-out/v2/gen/fixture/fedcba9876543210/stability.conf")
    val options = listOf("$root/$first", "$root/$second")
    record(optionValues = options, extraDigests = mapOf(first to "a", second to "b"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(
        record(optionValues = options, extraDigests = mapOf(first to "changed", second to "b"))
            .filesChanged,
    )
  }

  @Test
  fun `outputs and friend paths named by options are not compared`() {
    val output = root.resolve("buck-out/v2/tmp/kotlin-used-classes.json")
    val friend = root.resolve("friend.jar")
    Files.createDirectories(output.parent)
    listOf(output, friend).forEach { Files.writeString(it, "first") }
    val options = listOf("-P", "plugin:buck_deps_tracker:out=$output", "-Xfriend-paths=$friend")
    record(optionValues = options)
    IncrementalCompilerInputs.promotePending(stateDir)

    listOf(output, friend).forEach { Files.writeString(it, "other") }

    assertFalse(record(optionValues = options).filesChanged)
  }

  @Test
  fun `a file named by an option is compared by its content whatever its name`() {
    // Compose reads its stability configuration from any file, an archive name included.
    val input = Paths.get("buck-out/v2/gen/stability.jar")
    val option = listOf("-P", "plugin:androidx.compose:stabilityConfigurationPath=$root/$input")
    record(optionValues = option, extraDigests = mapOf(input to "first"))
    IncrementalCompilerInputs.promotePending(stateDir)
    assertTrue(record(optionValues = option, extraDigests = mapOf(input to "second")).filesChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    val sourceTreeFile = root.resolve("config.zip")
    Files.writeString(sourceTreeFile, "first")
    val sourceTreeOption = listOf("-P", "plugin:x:config=$sourceTreeFile")
    record(optionValues = sourceTreeOption)
    IncrementalCompilerInputs.promotePending(stateDir)
    Files.writeString(sourceTreeFile, "other")

    assertTrue(record(optionValues = sourceTreeOption).filesChanged)
  }

  @Test
  fun `a missing record counts as changed`() {
    record()
    IncrementalCompilerInputs.promotePending(stateDir)

    Files.delete(stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE))

    val changes = record()
    assertTrue(changes.filesChanged)
    assertTrue(changes.classpathOrderChanged)
    assertTrue(changes.moduleMappingsChanged)
  }

  @Test
  fun `without input tracking the mode is returned as it is and nothing is touched`() {
    // A working directory whose parent does not exist: any write would create it.
    val missingStateDir = root.resolve("missing_state")
    val mode = incrementalMode(missingStateDir, tracking = null)
    var compared = false

    val result =
        IncrementalCompilerInputs.apply(mode, arguments(), emptyList()) { _, _ -> compared = true }

    assertSame(mode, result)
    assertFalse(compared)
    assertFalse(Files.exists(missingStateDir))
  }

  @Test
  fun `with input tracking a changed record compiles in full or recomputes the classpath`() {
    val first =
        IncrementalCompilerInputs.apply(incrementalMode(stateDir), arguments(), emptyList()) { _, _
          ->
        }
    assertEquals(
        RebuildReason.COMPILER_INPUT_FILES_CHANGED,
        (first as KotlincMode.Incremental).rebuildReason,
    )
    IncrementalCompilerInputs.promotePending(stateDir)

    val unchanged =
        IncrementalCompilerInputs.apply(incrementalMode(stateDir), arguments(), emptyList()) { _, _
          ->
        }
    assertNull((unchanged as KotlincMode.Incremental).rebuildReason)
    assertTrue(unchanged.classpathChanges is ClasspathChanges.NoChanges)
    IncrementalCompilerInputs.promotePending(stateDir)

    val reordered =
        IncrementalCompilerInputs.apply(
            incrementalMode(stateDir),
            arguments(classpath = listOf("second.jar", "first.jar")),
            emptyList(),
        ) { _, _ ->
        }
    assertNull((reordered as KotlincMode.Incremental).rebuildReason)
    assertTrue(reordered.classpathChanges is ClasspathChanges.ToBeComputedByIncrementalCompiler)
  }

  @Test
  fun `a plugin path that cannot be parsed compiles in full and drops the committed record`() {
    IncrementalCompilerInputs.apply(incrementalMode(stateDir), arguments(), emptyList()) { _, _ -> }
    IncrementalCompilerInputs.promotePending(stateDir)
    val committed = stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE)
    assertTrue(Files.exists(committed))

    // A NUL character makes the path invalid, which is not an I/O failure.
    val result =
        IncrementalCompilerInputs.apply(
            incrementalMode(stateDir),
            arguments() + "-Xplugin=$root/invalid\u0000.jar",
            emptyList(),
        ) { _, _ ->
        }

    assertEquals(
        RebuildReason.INCREMENTAL_INPUTS_UNREADABLE,
        (result as KotlincMode.Incremental).rebuildReason,
    )
    assertFalse(Files.exists(committed))
  }

  private fun incrementalMode(
      incrementalStateDir: Path,
      tracking: IncrementalInputTracking? =
          IncrementalInputTracking(mapOf(Paths.get("plugin.jar") to "a")),
  ): KotlincMode.Incremental =
      KotlincMode.Incremental(
          AbsPath.of(root),
          AbsPath.of(root),
          AbsPath.of(incrementalStateDir.resolve("kotlinc_working_dir")),
          KotlinSourceChanges.ToBeCalculated,
          ClasspathChanges.NoChanges(emptyList()),
          null,
          null,
          tracking,
      )

  private fun arguments(
      classpath: List<String> = listOf("first.jar", "second.jar"),
  ): List<String> = listOf(
      "-classpath",
      classpath.joinToString(java.io.File.pathSeparator) { "$root/$it" },
      "-Xplugin=$root/plugin.jar",
      "-jvm-target",
      "1.8",
      "$root/src/A.kt",
  )

  private fun record(
      dependencyHash: String = "0123456789abcdef",
      pluginDigest: String? = "a",
      classpath: List<String> = listOf("first.jar"),
      optionValues: List<String> = emptyList(),
      extraDigests: Map<Path, String> = emptyMap(),
      sources: List<Path> = emptyList(),
  ): IncrementalCompilerInputs.Changes {
    val classpathArgument =
        (classpath.map { "$root/buck-out/v2/art/__$it/$dependencyHash/class-abi.jar" })
            .joinToString(java.io.File.pathSeparator)
    return IncrementalCompilerInputs.recordAndCompare(
        stateDir,
        root,
        listOf("-classpath", classpathArgument, "-Xplugin=$root/plugin.jar", "-jvm-target", "1.8") +
            optionValues +
            "$root/src/A.kt",
        (if (pluginDigest == null) emptyMap() else mapOf(Paths.get("plugin.jar") to pluginDigest)) +
            extraDigests,
        optionValues,
        sources,
    )
  }

  @Test
  fun `module mappings are read only from jars the record does not hold`() {
    val jar = root.resolve("buck-out/v2/art/__provider__/0123456789abcdef/provider.jar")
    writeJar(jar, mapOf("p/A.class" to "class", "META-INF/main.kotlin_module" to "mapping"))
    val relative = root.relativize(jar)

    // No record yet: every jar is read, and the mappings count as changed.
    val first = recordModules(listOf(jar), mapOf(relative to "1"))
    assertTrue(first.moduleMappingsChanged)
    assertEquals(1, first.moduleMappingJarsRead)
    IncrementalCompilerInputs.promotePending(stateDir)

    // The same path and digest need no read: a missing jar would fail otherwise.
    Files.delete(jar)
    val unchanged = recordModules(listOf(jar), mapOf(relative to "1"))
    assertFalse(unchanged.moduleMappingsChanged)
    assertEquals(0, unchanged.moduleMappingJarsRead)
    IncrementalCompilerInputs.promotePending(stateDir)

    // A new digest with the same mappings, then one without them.
    writeJar(jar, mapOf("p/A.class" to "other", "META-INF/main.kotlin_module" to "mapping"))
    assertFalse(recordModules(listOf(jar), mapOf(relative to "2")).moduleMappingsChanged)
    IncrementalCompilerInputs.promotePending(stateDir)
    writeJar(jar, mapOf("p/A.class" to "other"))
    assertTrue(recordModules(listOf(jar), mapOf(relative to "3")).moduleMappingsChanged)
  }

  @Test
  fun `a new content hash in the path of a jar with the same mappings is not a change`() {
    val before = root.resolve("buck-out/v2/art/__provider__/0123456789abcdef/provider.jar")
    val after = root.resolve("buck-out/v2/art/__provider__/fedcba9876543210/provider.jar")
    writeJar(before, mapOf("p/A.class" to "one", "META-INF/main.kotlin_module" to "mapping"))
    writeJar(after, mapOf("p/A.class" to "two", "META-INF/main.kotlin_module" to "mapping"))
    recordModules(listOf(before), mapOf(root.relativize(before) to "1"))
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(
        recordModules(listOf(after), mapOf(root.relativize(after) to "2")).moduleMappingsChanged,
    )
  }

  @Test
  fun `a jar that carries only module mappings entering or leaving the classpath is a change`() {
    val classes = root.resolve("classes.jar")
    val mappingsOnly = root.resolve("mappings.jar")
    writeJar(classes, mapOf("p/A.class" to "class"))
    writeJar(mappingsOnly, mapOf("META-INF/main.kotlin_module" to "mapping"))
    val digests = mapOf(Paths.get("classes.jar") to "1", Paths.get("mappings.jar") to "1")
    recordModules(listOf(classes), digests)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(recordModules(listOf(classes, mappingsOnly), digests).moduleMappingsChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertTrue(recordModules(listOf(classes), digests).moduleMappingsChanged)
  }

  @Test
  fun `a jar without module mappings entering the classpath is not a module mappings change`() {
    val first = root.resolve("first.jar")
    val second = root.resolve("second.jar")
    writeJar(first, mapOf("META-INF/first.kotlin_module" to "first"))
    writeJar(second, mapOf("p/B.class" to "class"))
    val digests = mapOf(Paths.get("first.jar") to "1", Paths.get("second.jar") to "1")
    recordModules(listOf(first), digests)
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(recordModules(listOf(first, second), digests).moduleMappingsChanged)
  }

  @Test
  fun `a jar without an input digest is read every round where no stored digest exists`() {
    // The temporary directory keeps no content digests, unlike an EdenFS checkout.
    val jar = root.resolve("toolchain.jar")
    writeJar(jar, mapOf("META-INF/main.kotlin_module" to "mapping-1"))
    val time = Files.getLastModifiedTime(jar)
    assertEquals(1, recordModules(listOf(jar), emptyMap()).moduleMappingJarsRead)
    IncrementalCompilerInputs.promotePending(stateDir)

    val unchanged = recordModules(listOf(jar), emptyMap())
    assertEquals(1, unchanged.moduleMappingJarsRead)
    assertFalse(unchanged.moduleMappingsChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    // Other mappings of the same size, with the same modification time.
    writeJar(jar, mapOf("META-INF/main.kotlin_module" to "mapping-2"))
    Files.setLastModifiedTime(jar, time)
    assertTrue(recordModules(listOf(jar), emptyMap()).moduleMappingsChanged)
  }

  @Test
  fun `module mappings in a classpath directory are compared`() {
    val directory = root.resolve("buck-out/v2/art/dep/__abi_dir__")
    val module = directory.resolve("META-INF/main.kotlin_module")
    Files.createDirectories(module.parent)
    Files.writeString(module, "mapping-1")
    recordModules(listOf(directory), emptyMap())
    IncrementalCompilerInputs.promotePending(stateDir)

    assertFalse(recordModules(listOf(directory), emptyMap()).moduleMappingsChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    Files.writeString(module, "mapping-2")
    assertTrue(recordModules(listOf(directory), emptyMap()).moduleMappingsChanged)
    IncrementalCompilerInputs.promotePending(stateDir)

    Files.delete(module)
    assertTrue(recordModules(listOf(directory), emptyMap()).moduleMappingsChanged)
  }

  @Test
  fun `a jar whose module mappings cannot be read deletes the committed record`() {
    val jar = root.resolve("dep.jar")
    writeJar(jar, mapOf("META-INF/main.kotlin_module" to "mapping"))
    recordModules(listOf(jar), emptyMap())
    IncrementalCompilerInputs.promotePending(stateDir)

    Files.writeString(jar, "not a zip")
    assertThrows(IOException::class.java) { recordModules(listOf(jar), emptyMap()) }

    assertFalse(Files.exists(stateDir.resolve(IncrementalCompilerInputs.RECORD_FILE)))
  }

  private fun recordModules(
      classpath: List<Path>,
      digests: Map<Path, String>,
  ): IncrementalCompilerInputs.Changes =
      IncrementalCompilerInputs.recordAndCompare(
          stateDir,
          root,
          listOf("-classpath", classpath.joinToString(java.io.File.pathSeparator)),
          digests,
          emptyList(),
      )

  private fun writeJar(jar: Path, entries: Map<String, String>) {
    Files.createDirectories(jar.parent)
    ZipOutputStream(Files.newOutputStream(jar)).use { zip ->
      for ((name, content) in entries) {
        zip.putNextEntry(ZipEntry(name))
        zip.write(content.toByteArray())
        zip.closeEntry()
      }
    }
  }
}
