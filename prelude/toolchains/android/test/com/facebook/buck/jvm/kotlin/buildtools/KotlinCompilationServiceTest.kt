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

import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.jvm.java.CompilerClassToSourceMap
import com.facebook.buck.jvm.kotlin.buildtools.snapshot.SnapshotGranularity
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.ClasspathChanges
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode
import com.facebook.buck.testutil.TemporaryPaths
import com.google.common.collect.ImmutableList
import java.io.File
import java.nio.file.Files
import java.nio.file.attribute.FileTime
import java.util.UUID
import java.util.jar.JarFile
import javax.tools.ToolProvider
import kotlin.io.path.absolute
import kotlin.io.path.extension
import kotlin.io.path.listDirectoryEntries
import org.jetbrains.kotlin.buildtools.api.CompilationResult
import org.jetbrains.kotlin.buildtools.api.CompilationService
import org.jetbrains.kotlin.buildtools.api.ExperimentalBuildToolsApi
import org.jetbrains.kotlin.buildtools.api.ProjectId
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.mockito.kotlin.mock

// Legacy BTA API is DEPRECATION_ERROR from Kotlin 2.4; KotlinToolchains replaces it.
@Suppress("DEPRECATION_ERROR")
@OptIn(ExperimentalBuildToolsApi::class)
internal class KotlinCompilationServiceTest {

  @JvmField @Rule val temporaryFolder: TemporaryPaths = TemporaryPaths(false)

  private lateinit var kotlinCompilationService: KotlinCompilationService

  private lateinit var sourcesDir: AbsPath
  private lateinit var classesDir: AbsPath
  private lateinit var incrementalDir: AbsPath
  private lateinit var kotlinDepFile: AbsPath
  private lateinit var jvmAbiGenWorkingDir: AbsPath
  private lateinit var fooSourceFile: KotlinSourceFile
  private lateinit var barSourceFile: KotlinSourceFile

  private lateinit var librariesDir: AbsPath
  private lateinit var librarySnapshotsDir: AbsPath

  @Before
  fun setUp() {
    kotlinCompilationService =
        KotlinCompilationService(
            CompilationService.loadImplementation(this::class.java.classLoader),
            mock(),
        )

    sourcesDir = temporaryFolder.newFolder("src")
    classesDir = temporaryFolder.newFolder("__classes__")
    kotlinDepFile = temporaryFolder.newFile("kotlin-used-classes.json")
    jvmAbiGenWorkingDir = temporaryFolder.newFolder("jvm_abi_gen_working_dir")
    incrementalDir = temporaryFolder.newFolder("incrementalState")

    librariesDir = temporaryFolder.newFolder("libraries")
    librarySnapshotsDir = temporaryFolder.newFolder("library_snapshots")

    fooSourceFile =
        KotlinSourceFile(
            sourcesDir.resolve("Foo.kt"),
            """
            |class Foo {
            |
            | var counter: Int = 0
            |
            | fun foo() {
            |   counter = 1
            | }
            |
            | fun bar() {}
            |}
            |"""
                .trimMargin(),
        )

    barSourceFile =
        KotlinSourceFile(
            sourcesDir.resolve("Bar.kt"),
            """
            |class Bar(private val foo: Foo) {
            |  fun bar() {
            |    foo.foo()
            |  }
            |}
            |"""
                .trimMargin(),
        )
  }

  @Test
  fun `when compile non incrementally, all source files are recompiled`() {
    assertEquals(true, classesDir.path.listDirectoryEntries().isEmpty())

    val result =
        kotlinCompilationService.compile(
            projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
            arguments =
                createCompilerArgs(
                    sourceFiles = listOf(fooSourceFile, barSourceFile),
                    outputDir = classesDir,
                ),
            mode = KotlincMode.NonIncremental,
        )

    assertEquals(CompilationResult.COMPILATION_SUCCESS, result)
    val classes = classesDir.path.listDirectoryEntries().filter { it.extension == "class" }
    assertEquals(classes.size, 2)
    assertTrue(classes.any { it.endsWith("Foo.class") })
    assertTrue(classes.any { it.endsWith("Bar.class") })
  }

  @Test
  fun `when compile incrementally, all source files are recompiled on the first run`() {
    assertEquals(true, incrementalDir.path.listDirectoryEntries().isEmpty())

    val result =
        kotlinCompilationService.compile(
            projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
            arguments =
                createCompilerArgs(
                    sourceFiles = listOf(fooSourceFile, barSourceFile),
                    outputDir = classesDir,
                ),
            mode = createIncrementalMode(),
        )

    assertEquals(CompilationResult.COMPILATION_SUCCESS, result)
    assertTrue(incrementalDir.path.listDirectoryEntries().isNotEmpty())
    val classes = classesDir.path.listDirectoryEntries().filter { it.extension == "class" }
    assertEquals(2, classes.size)
    assertTrue(classes.any { it.endsWith("Foo.class") })
    assertTrue(classes.any { it.endsWith("Bar.class") })
  }

  @Test
  fun `when a non-abi change in Foo, only Foo gets recompiled`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile, barSourceFile),
                outputDir = classesDir,
            ),
        mode = createIncrementalMode(),
    )
    resetClassModificationTimesToSentinel()
    val initialClassTimestamps = getClassModificationTimes()
    fooSourceFile.changeContent("counter = 1", "counter = 2")
    val result =
        kotlinCompilationService.compile(
            projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
            arguments =
                createCompilerArgs(
                    sourceFiles = listOf(fooSourceFile, barSourceFile),
                    outputDir = classesDir,
                ),
            mode = createIncrementalMode(),
        )

    assertEquals(CompilationResult.COMPILATION_SUCCESS, result)
    val postCompilationTimestamps = getClassModificationTimes()
    assertNotEquals(initialClassTimestamps["Foo.class"], postCompilationTimestamps["Foo.class"])
    assertEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
  }

  @Test
  fun `when a abi change in Foo, all affected files gets recompiled`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile, barSourceFile),
                outputDir = classesDir,
            ),
        mode = createIncrementalMode(),
    )
    resetClassModificationTimesToSentinel()
    val initialClassTimestamps = getClassModificationTimes()
    fooSourceFile.changeContent("foo()", "foo(i: Int = 1)")

    val result =
        kotlinCompilationService.compile(
            projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
            arguments =
                createCompilerArgs(
                    sourceFiles = listOf(fooSourceFile, barSourceFile),
                    outputDir = classesDir,
                ),
            mode = createIncrementalMode(),
        )

    assertEquals(CompilationResult.COMPILATION_SUCCESS, result)
    val postCompilationTimestamps = getClassModificationTimes()
    assertNotEquals(initialClassTimestamps["Foo.class"], postCompilationTimestamps["Foo.class"])
    assertNotEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
  }

  @Test
  fun `when a compilation error in Foo, its output is restored`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile, barSourceFile),
                outputDir = classesDir,
            ),
        mode = createIncrementalMode(),
    )
    val initialClassTimestamps = getClassModificationTimes()
    fooSourceFile.changeContent("foo()", "foo(i: String = 1)")

    val result =
        kotlinCompilationService.compile(
            projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
            arguments =
                createCompilerArgs(
                    sourceFiles = listOf(fooSourceFile, barSourceFile),
                    outputDir = classesDir,
                ),
            mode = createIncrementalMode(),
        )

    assertEquals(CompilationResult.COMPILATION_ERROR, result)
    val postCompilationTimestamps = getClassModificationTimes()
    assertEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
    assertEquals(initialClassTimestamps["Foo.class"], postCompilationTimestamps["Foo.class"])
  }

  @Test
  fun `when a non-abi change in a Foo library, Bar does not get recompiled`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    var snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    val initialClassTimestamps = getClassModificationTimes()

    fooSourceFile.changeContent("counter = 1", "counter = 2")
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    val postCompilationTimestamps = getClassModificationTimes()

    assertEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
  }

  @Test
  fun `when an abi change in a Foo library that impacts Bar, Bar gets recompiled`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    var snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    resetClassModificationTimesToSentinel()
    val initialClassTimestamps = getClassModificationTimes()
    fooSourceFile.changeContent("foo()", "foo(i: Int = 1)")
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    val postCompilationTimestamps = getClassModificationTimes()

    assertNotEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
  }

  @Test
  fun `when an abi change in a Foo library that does not impact Bar and CLASS_MEMBER_LEVEL snapshot is used, Bar does not get recompiled`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    var snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    val initialClassTimestamps = getClassModificationTimes()

    fooSourceFile.changeContent("fun bar() {}", "")
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    val postCompilationTimestamps = getClassModificationTimes()

    assertEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
  }

  @Test
  fun `when an abi change in a Foo library that does not impact Bar and CLASS_LEVEL snapshot is used, Bar gets recompiled`() {
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    var snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    resetClassModificationTimesToSentinel()
    val initialClassTimestamps = getClassModificationTimes()
    fooSourceFile.changeContent("fun bar() {}", "")
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(fooSourceFile),
                outputDir = librariesDir,
            ),
        mode = KotlincMode.NonIncremental,
    )
    snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_LEVEL)
    kotlinCompilationService.compile(
        projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
        arguments =
            createCompilerArgs(
                sourceFiles = listOf(barSourceFile),
                outputDir = classesDir,
                classpath = listOf(librariesDir),
            ),
        mode =
            createIncrementalMode(
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
            ),
    )
    val postCompilationTimestamps = getClassModificationTimes()

    assertNotEquals(initialClassTimestamps["Bar.class"], postCompilationTimestamps["Bar.class"])
  }

  @Test
  fun `failed compilation does not publish a source map`() {
    val broken = KotlinSourceFile(sourcesDir.resolve("Broken.kt"), "class Broken {")
    val output = temporaryFolder.root.resolve("class-to-source.json").path
    fun compile(): CompilationResult =
        kotlinCompilationService.compile(
            ProjectId.ProjectUUID(UUID.randomUUID()),
            createCompilerArgs(listOf(broken), classesDir),
            KotlincMode.NonIncremental,
            classToSourceMap = KotlinClassToSourceMap(output, classesDir.path, sourcesDir.path),
        )
    assertEquals(CompilationResult.COMPILATION_ERROR, compile())
    assertEquals(false, Files.exists(output))

    Files.writeString(output, "previous successful report")
    assertEquals(CompilationResult.COMPILATION_ERROR, compile())
    assertEquals("previous successful report", Files.readString(output))
  }

  @Test
  fun `compiler maps bodyless data classes from a differently named file`() {
    val models = KotlinSourceFile(
        sourcesDir.resolve("Models.kt"),
        "package example; data class Settings(val value: Int); data class Profile(val enabled: Boolean)",
    )
    val output = temporaryFolder.root.resolve("class-to-source.json").path
    val result =
        kotlinCompilationService.compile(
            projectId = ProjectId.ProjectUUID(UUID.randomUUID()),
            arguments = createCompilerArgs(listOf(models), classesDir),
            mode = KotlincMode.NonIncremental,
            classToSourceMap = KotlinClassToSourceMap(output, classesDir.path, sourcesDir.path),
        )
    assertEquals(CompilationResult.COMPILATION_SUCCESS, result)
    assertEquals(
        mapOf(
            "example/Settings.class" to setOf("Models.kt"),
            "example/Profile.class" to setOf("Models.kt"),
        ),
        CompilerClassToSourceMap.read(output).entries,
    )
  }

  @Test
  fun `incremental maps retain unchanged classes and remove deleted classes`() {
    val output = temporaryFolder.root.resolve("mapping.json").path
    val state = incrementalDir.resolve("class-to-source.json").path
    fun compile(sources: List<KotlinSourceFile>): CompilationResult =
        kotlinCompilationService.compile(
            ProjectId.ProjectUUID(UUID.randomUUID()),
            createCompilerArgs(sources, classesDir),
            createIncrementalMode(),
            classToSourceMap =
                KotlinClassToSourceMap(output, classesDir.path, sourcesDir.path, state),
        )
    assertEquals(
        CompilationResult.COMPILATION_SUCCESS,
        compile(listOf(fooSourceFile, barSourceFile)),
    )
    val expected = mapOf("Foo.class" to setOf("Foo.kt"), "Bar.class" to setOf("Bar.kt"))
    assertEquals(expected, CompilerClassToSourceMap.read(output).entries)
    resetClassModificationTimesToSentinel()
    fooSourceFile.changeContent("counter = 1", "counter = 2")
    assertEquals(
        CompilationResult.COMPILATION_SUCCESS,
        compile(listOf(fooSourceFile, barSourceFile)),
    )
    assertEquals(
        SENTINEL_TIMESTAMP_MS,
        Files.getLastModifiedTime(classesDir.resolve("Bar.class").path).toMillis(),
    )
    assertEquals(expected, CompilerClassToSourceMap.read(output).entries)
    assertEquals(
        CompilationResult.COMPILATION_SUCCESS,
        compile(listOf(fooSourceFile, barSourceFile)),
    )
    assertEquals(expected, CompilerClassToSourceMap.read(output).entries)
    Files.delete(barSourceFile.absPath.path)
    assertEquals(CompilationResult.COMPILATION_SUCCESS, compile(listOf(fooSourceFile)))
    assertEquals(
        mapOf("Foo.class" to setOf("Foo.kt")),
        CompilerClassToSourceMap.read(output).entries,
    )
    Files.delete(state)
    assertEquals(CompilationResult.COMPILATION_SUCCESS, compile(listOf(fooSourceFile)))
    assertEquals(
        mapOf("Foo.class" to setOf("Foo.kt")),
        CompilerClassToSourceMap.read(output).entries,
    )
    for (invalidState in listOf("{\"", "null", "{\"Foo.class\":null}", "{\"Foo.class\":[null]}")) {
      Files.writeString(state, invalidState)
      resetClassModificationTimesToSentinel()
      assertEquals(CompilationResult.COMPILATION_SUCCESS, compile(listOf(fooSourceFile)))
      assertNotEquals(
          SENTINEL_TIMESTAMP_MS,
          Files.getLastModifiedTime(classesDir.resolve("Foo.class").path).toMillis(),
      )
      assertEquals(
          mapOf("Foo.class" to setOf("Foo.kt")),
          CompilerClassToSourceMap.read(output).entries,
      )
      assertEquals(
          CompilerClassToSourceMap.read(output).entries,
          CompilerClassToSourceMap.read(state).entries,
      )
    }
  }

  @Test
  fun `prebuilt snapshot generator selects the registered compiler version`() {
    val compilerVersion = checkNotNull(System.getenv("KOTLIN_COMPILER_VERSION"))
    assertEquals(
        compilerVersion,
        CompilationService.loadImplementation(javaClass.classLoader).getCompilerVersion(),
    )
    val expectedGeneratorVersion =
        if (compilerVersion == "2.5.0-Beta2") compilerVersion else "2.0.20"
    val generatorVersion =
        JarFile(checkNotNull(System.getenv("CP_SNAPSHOT_GENERATOR_JAR"))).use { jar ->
          jar.getInputStream(jar.getJarEntry("META-INF/compiler.version")).bufferedReader().use {
            it.readText().trim()
          }
        }
    assertEquals(expectedGeneratorVersion, generatorVersion)
  }

  @Test
  fun `prebuilt Java snapshots are consumed incrementally`() {
    val javaSource = sourcesDir.resolve("JavaDependency.java")
    val consumer = KotlinSourceFile(
        sourcesDir.resolve("Consumer.kt"),
        "class Consumer { fun value() = JavaDependency().value() }",
    )
    fun compileDependency(returnType: String) {
      Files.writeString(
          javaSource.path,
          "public class JavaDependency { public $returnType value() { return 1; } }",
      )
      assertEquals(
          0,
          checkNotNull(ToolProvider.getSystemJavaCompiler())
              .run(null, null, null, "-d", librariesDir.toString(), javaSource.toString()),
      )
    }
    fun compileConsumer(): CompilationResult {
      val snapshot = generateClasspathSnapshot(librariesDir, SnapshotGranularity.CLASS_MEMBER_LEVEL)
      return kotlinCompilationService.compile(
          ProjectId.ProjectUUID(UUID.randomUUID()),
          createCompilerArgs(listOf(consumer), classesDir, listOf(librariesDir)),
          createIncrementalMode(
              ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of(snapshot)),
          ),
      )
    }

    compileDependency("int")
    assertEquals(CompilationResult.COMPILATION_SUCCESS, compileConsumer())
    resetClassModificationTimesToSentinel()
    assertEquals(CompilationResult.COMPILATION_SUCCESS, compileConsumer())
    assertEquals(SENTINEL_TIMESTAMP_MS, getClassModificationTimes()["Consumer.class"])

    compileDependency("long")
    assertEquals(CompilationResult.COMPILATION_SUCCESS, compileConsumer())
    assertNotEquals(SENTINEL_TIMESTAMP_MS, getClassModificationTimes()["Consumer.class"])
  }

  private fun createCompilerArgs(
      sourceFiles: List<KotlinSourceFile>,
      outputDir: AbsPath,
      classpath: List<AbsPath> = emptyList(),
  ): List<String> {
    // The compiler is invoked with `-no-stdlib`, so the stdlib must be supplied explicitly on the
    // classpath. It has to be a standalone kotlin-stdlib jar: resolving it reflectively from
    // `KotlinVersion` yields the merged test binary, whose `META-INF/*.kotlin_module` metadata is
    // stripped, leaving top-level declarations such as `println` unresolvable. The path is injected
    // by the `KOTLIN_STDLIB_JAR` env var set on the test target.
    val stdlibLocation =
        AbsPath.of(
            File(
                checkNotNull(System.getenv(KOTLIN_STDLIB_JAR_ENV)) {
                  "$KOTLIN_STDLIB_JAR_ENV env var is not set"
                },
            )
                .toPath()
                .absolute(),
        )

    return buildList {
      add("-no-reflect")
      add("-no-stdlib")
      add("-d")
      add(outputDir.toString())
      add("-cp")
      add((classpath + stdlibLocation).joinToString(":"))
      add("-module-name")
      add(temporaryFolder.root.path.fileName.toString())
      addAll(sourceFiles.map { sourceFile -> sourceFile.absPath.toString() })
    }
  }

  private fun createIncrementalMode(
      classPathChanges: ClasspathChanges = ClasspathChanges.NoChanges(ImmutableList.of()),
  ) =
      KotlincMode.Incremental(
          rootProjectDir = temporaryFolder.root,
          buildDir = temporaryFolder.root,
          kotlicWorkingDir = incrementalDir,
          kotlinSourceChanges = KotlinSourceChanges.ToBeCalculated,
          classpathChanges = classPathChanges,
          kotlinClassUsageFile = kotlinDepFile,
          rebuildReason = null,
      )

  private fun getClassModificationTimes(): Map<String, Long> =
      classesDir.path.listDirectoryEntries().associate { path ->
        path.fileName.toString() to Files.getLastModifiedTime(path).toMillis()
      }

  /**
   * Stamp existing `.class` outputs with a fixed past mtime so a later recompile (which writes a
   * current mtime) is detected as `mtime != sentinel`, independent of filesystem timestamp
   * granularity. Call after the first compile, before capturing the baseline timestamps.
   */
  private fun resetClassModificationTimesToSentinel() {
    val sentinel = FileTime.fromMillis(SENTINEL_TIMESTAMP_MS)
    classesDir.path.listDirectoryEntries().forEach { path ->
      Files.setLastModifiedTime(path, sentinel)
    }
  }

  private fun generateClasspathSnapshot(
      dependencyOutput: AbsPath,
      granularity: SnapshotGranularity,
  ): File {
    // `CachedClasspathSnapshotSerializer` caches snapshots by file name (see its docs), so every
    // snapshot needs a distinct name or a stale cached snapshot is returned. A timestamp-based name
    // collides when two snapshots are generated within the same millisecond, so use a UUID instead.
    val snapshotFile = librarySnapshotsDir.resolve("dep-${UUID.randomUUID()}.snapshot")
    // Exercise the bootstrap prebuilt, not the in-process generator whose compiler always matches.
    val generatorJar = checkNotNull(System.getenv("CP_SNAPSHOT_GENERATOR_JAR"))
    val process = ProcessBuilder(
        File(System.getProperty("java.home"), "bin/java").path,
        "-jar",
        generatorJar,
        "--input-jar",
        dependencyOutput.toString(),
        "--output-snapshot",
        snapshotFile.toString(),
        "--granularity",
        granularity.name,
    )
        .redirectErrorStream(true)
        .start()
    val output = process.inputStream.bufferedReader().use { it.readText() }
    assertEquals(output, 0, process.waitFor())

    return snapshotFile.toFile()
  }

  companion object {
    private const val KOTLIN_STDLIB_JAR_ENV = "KOTLIN_STDLIB_JAR"

    // 2000-01-01T00:00:00Z.
    private const val SENTINEL_TIMESTAMP_MS = 946_684_800_000L
  }
}
