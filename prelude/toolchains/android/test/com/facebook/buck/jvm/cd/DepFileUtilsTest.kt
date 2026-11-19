/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.cd

import com.facebook.buck.util.json.ObjectMappers
import com.fasterxml.jackson.core.type.TypeReference
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.Paths
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class DepFileUtilsTest {

  @get:Rule val tempFolder = TemporaryFolder()

  @Test
  fun `when usedClassesToDepFile is called then it produces sorted output`() {
    val usedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class1.class"),
            "/path/to/A.jar" to setOf("ClassA.class"),
            "/path/to/B.jar" to setOf("ClassB.class"),
        ),
    )
    val depFileOutput = tempFolder.newFile("dep-file.txt").toPath()

    DepFileUtils.usedClassesToDepFile(
        listOf(usedClassesMapPath),
        depFileOutput,
        emptyMap(),
        false,
    )

    val outputLines = Files.readAllLines(depFileOutput)
    val expectedSortedPaths = listOf("/path/to/A.jar", "/path/to/B.jar", "/path/to/C.jar")
    assertEquals(expectedSortedPaths, outputLines)
  }

  @Test
  fun `when usedClassesToDepFile is called with a previous output then it includes it`() {
    val prevUsedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class1.class"),
            "/path/to/A.jar" to setOf("ClassA.class"),
            "/path/to/B.jar" to setOf("ClassB.class"),
        ),
        "prev-used-classes.json",
    )
    val depFileOutput = tempFolder.newFile("dep-file.txt").toPath()
    DepFileUtils.usedClassesToDepFile(
        listOf(prevUsedClassesMapPath),
        depFileOutput,
        emptyMap(),
        false,
    )

    val usedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class1.class"),
            "/path/to/A.jar" to setOf("ClassA.class"),
            "/path/to/D.jar" to setOf("ClassD.class"),
        ),
    )

    DepFileUtils.usedClassesToDepFile(
        listOf(usedClassesMapPath),
        depFileOutput,
        emptyMap(),
        true,
    )

    val outputLines = Files.readAllLines(depFileOutput)
    val expectedSortedPaths =
        listOf("/path/to/A.jar", "/path/to/B.jar", "/path/to/C.jar", "/path/to/D.jar")
    assertEquals(expectedSortedPaths, outputLines)
  }

  @Test
  fun `when usedClassesToDepFile is called with jarToJarDirMap then it produces sorted output`() {
    val usedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class1.class"),
            "/path/to/A.jar" to setOf("ClassZ.class"),
            "/path/to/B.jar" to setOf("ClassB.class"),
        ),
    )
    val jarToJarDirMap = mapOf(
        Paths.get("/path/to/C.jar") to Paths.get("/expanded/C"),
        Paths.get("/path/to/A.jar") to Paths.get("/expanded/A"),
    )
    val depFileOutput = tempFolder.newFile("dep-file.txt").toPath()

    DepFileUtils.usedClassesToDepFile(
        listOf(usedClassesMapPath),
        depFileOutput,
        jarToJarDirMap,
        false,
    )

    val outputLines = Files.readAllLines(depFileOutput)
    val expectedSortedPaths = listOf(
        "/expanded/A/ClassZ.class",
        "/expanded/C/Class1.class",
        "/expanded/C/Class3.class",
        "/path/to/B.jar",
    )
    assertEquals(expectedSortedPaths, outputLines)
  }

  @Test
  fun `when usedClassesToUsedJars is called then it produces sorted output`() {
    val usedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class1.class"),
            "/path/to/A.jar" to setOf("ClassA.class"),
            "/path/to/B.jar" to setOf("ClassB.class"),
        ),
    )
    val usedJarsOutput = tempFolder.newFile("used-jars.txt").toPath()

    DepFileUtils.usedClassesToUsedJars(listOf(usedClassesMapPath), usedJarsOutput, false)

    val outputLines = Files.readAllLines(usedJarsOutput).joinToString("\n")
    val actual =
        ObjectMappers.readValue(
            outputLines,
            object : TypeReference<LinkedHashMap<String, List<String>>>() {},
        )
    val expected = mapOf(
        "/path/to/A.jar" to listOf("ClassA.class"),
        "/path/to/B.jar" to listOf("ClassB.class"),
        "/path/to/C.jar" to listOf("Class1.class", "Class3.class"),
    )
    assertEquals(expected, actual)
  }

  @Test
  fun `when usedClassesToUsedJars is called with previous output it merges it`() {
    val prevUsedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class2.class"),
            "/path/to/A.jar" to setOf("ClassA.class"),
            "/path/to/B.jar" to setOf("ClassB.class"),
        ),
    )
    val usedJarsOutput = tempFolder.newFile("used-jars.txt").toPath()

    DepFileUtils.usedClassesToUsedJars(
        listOf(prevUsedClassesMapPath),
        usedJarsOutput,
        false,
    )

    val usedClassesMapPath = createUsedClassesJson(
        mapOf(
            "/path/to/C.jar" to setOf("Class3.class", "Class1.class"),
            "/path/to/A.jar" to setOf("ClassA.class"),
            "/path/to/B.jar" to setOf("ClassB.class"),
        ),
        "prev-used-classes.json",
    )

    DepFileUtils.usedClassesToUsedJars(
        listOf(usedClassesMapPath),
        usedJarsOutput,
        true,
    )

    val outputLines = Files.readAllLines(usedJarsOutput).joinToString("\n")
    val actual =
        ObjectMappers.readValue(
            outputLines,
            object : TypeReference<LinkedHashMap<String, List<String>>>() {},
        )
    val expected = mapOf(
        "/path/to/A.jar" to listOf("ClassA.class"),
        "/path/to/B.jar" to listOf("ClassB.class"),
        "/path/to/C.jar" to listOf("Class1.class", "Class2.class", "Class3.class"),
    )
    assertEquals(expected, actual)
  }

  private fun createUsedClassesJson(
      classesMap: Map<String, Set<String>>,
      fileName: String = "used-classes.json",
  ): Path {
    val file = tempFolder.newFile(fileName)
    val mappedInput =
        classesMap.entries.associate { (jarPath, classes) ->
          Paths.get(jarPath) to classes.map { Paths.get(it) }.toSet()
        }

    ObjectMappers.WRITER.writeValue(file, mappedInput)
    return file.toPath()
  }
}
