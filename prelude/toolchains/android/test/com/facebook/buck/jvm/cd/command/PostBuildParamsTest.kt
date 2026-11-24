/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd.command

import com.facebook.buck.core.filesystems.RelPath
import com.facebook.buck.jvm.java.JarParameters
import com.google.common.collect.ImmutableSortedSet
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import java.util.Optional
import java.util.function.Predicate
import java.util.logging.Level
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class PostBuildParamsTest {

  @get:Rule val tempFolder = TemporaryFolder()

  @Test
  fun `class names file lists the class entries of the jar in entry order`() {
    val jar = writeJar(
        "lib.jar",
        "com/a/",
        "com/a/B.class",
        "com/a/res.txt",
        "META-INF/MANIFEST.MF",
        "com/ü/Ü.class",
        "com/a/A\$Inner.class",
        "module-info.class",
        "META-INF/versions/9/x/Y.class",
    )
    val output = tempFolder.root.toPath().resolve("class_names.txt")

    postBuildParams(output).maybeWriteClassNamesFile(jarParameters(jar))

    assertEquals(
        "com/a/B\ncom/ü/Ü\ncom/a/A\$Inner\nmodule-info\nMETA-INF/versions/9/x/Y\n",
        String(Files.readAllBytes(output), Charsets.UTF_8),
    )
  }

  @Test
  fun `class names file of a jar without classes is empty`() {
    val jar = writeJar("empty.jar", "META-INF/MANIFEST.MF")
    val output = tempFolder.root.toPath().resolve("class_names.txt")

    postBuildParams(output).maybeWriteClassNamesFile(jarParameters(jar))

    assertEquals(0L, Files.size(output))
  }

  @Test
  fun `class names file replaces a read-only file from an earlier build`() {
    val jar = writeJar("lib.jar", "com/a/B.class")
    val output = tempFolder.root.toPath().resolve("class_names.txt")
    Files.write(output, "stale\n".toByteArray())
    output.toFile().setReadOnly()

    postBuildParams(output).maybeWriteClassNamesFile(jarParameters(jar))

    assertEquals("com/a/B\n", String(Files.readAllBytes(output), Charsets.UTF_8))
  }

  @Test
  fun `nothing is written without a class names file`() {
    postBuildParams(null).maybeWriteClassNamesFile(null)

    assertEquals(0, tempFolder.root.list()!!.size)
  }

  private fun writeJar(name: String, vararg entries: String): Path {
    val jar = tempFolder.root.toPath().resolve(name)
    ZipOutputStream(Files.newOutputStream(jar)).use { zip ->
      for (entry in entries) {
        zip.putNextEntry(ZipEntry(entry))
        zip.closeEntry()
      }
    }
    return jar
  }

  // JarParameters takes a jar path relative to the working directory.
  private fun jarParameters(jar: Path): JarParameters = JarParameters(
      hashEntries = false,
      mergeManifests = false,
      jarPath = RelPath.of(Paths.get("").toAbsolutePath().relativize(jar.toAbsolutePath())),
      removeEntryPredicate = Predicate { false },
      entriesToJar = ImmutableSortedSet.of(),
      overrideEntriesToJar = ImmutableSortedSet.of(),
      mainClass = Optional.empty(),
      manifestFile = Optional.empty(),
      duplicatesLogLevel = Level.INFO,
  )

  private fun postBuildParams(classNamesFile: Path?): PostBuildParams = PostBuildParams(
      libraryJar = null,
      abiJar = null,
      jvmAbiGen = null,
      abiOutputDir = null,
      usedClassesPaths = listOf(),
      depFile = null,
      optionalDirsPaths = listOf(),
      incrementalStateDir = null,
      shouldCreateClassAbi = false,
      usedJarsPath = null,
      postProcessorCmd = null,
      filesWhichSkippedCompilation = null,
      jvmAbiFilesWhichSkippedCompilation = null,
      classNamesFile = classNamesFile,
  )
}
