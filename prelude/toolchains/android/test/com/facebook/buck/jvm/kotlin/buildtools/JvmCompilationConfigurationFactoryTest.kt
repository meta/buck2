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
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.ClasspathChanges
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlinSourceChanges
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.KotlincMode
import com.facebook.buck.jvm.kotlin.kotlinc.incremental.RebuildReason
import com.google.common.collect.ImmutableList
import java.io.File
import org.jetbrains.kotlin.buildtools.api.CompilationService
import org.jetbrains.kotlin.buildtools.api.ExperimentalBuildToolsApi
import org.jetbrains.kotlin.buildtools.api.SourcesChanges
import org.jetbrains.kotlin.buildtools.api.jvm.ClasspathSnapshotBasedIncrementalCompilationApproachParameters
import org.jetbrains.kotlin.buildtools.api.jvm.ClasspathSnapshotBasedIncrementalJvmCompilationConfiguration
import org.jetbrains.kotlin.buildtools.api.jvm.JvmCompilationConfiguration
import org.junit.Before
import org.junit.Test
import org.mockito.kotlin.any
import org.mockito.kotlin.argThat
import org.mockito.kotlin.eq
import org.mockito.kotlin.mock
import org.mockito.kotlin.never
import org.mockito.kotlin.verify
import org.mockito.kotlin.whenever

// Legacy BTA API is DEPRECATION_ERROR from Kotlin 2.4; KotlinToolchains replaces it.
@Suppress("DEPRECATION_ERROR")
@OptIn(ExperimentalBuildToolsApi::class)
internal class JvmCompilationConfigurationFactoryTest {

  private lateinit var jvmCompilationConfigurationFactory: JvmCompilationConfigurationFactory

  private val compilationService: CompilationService = mock()
  private val jvmCompilationConfiguration: JvmCompilationConfiguration = mock()
  private val classpathSnapshotBasedIncrementalJvmCompilationConfiguration:
      ClasspathSnapshotBasedIncrementalJvmCompilationConfiguration =
      mock()

  @Before
  fun setUp() {
    whenever(compilationService.makeJvmCompilationConfiguration())
        .thenReturn(jvmCompilationConfiguration)
    whenever(
        jvmCompilationConfiguration.makeClasspathSnapshotBasedIncrementalCompilationConfiguration(),
    )
        .thenReturn(classpathSnapshotBasedIncrementalJvmCompilationConfiguration)

    jvmCompilationConfigurationFactory =
        JvmCompilationConfigurationFactory(compilationService, mock())
  }

  @Test
  fun `when compiling with non-incremental mode, incremental configuration is not set`() {
    jvmCompilationConfigurationFactory.create(KotlincMode.NonIncremental)

    verify(compilationService).makeJvmCompilationConfiguration()
    verify(jvmCompilationConfiguration, never())
        .useIncrementalCompilation(
            workingDirectory = any(),
            sourcesChanges = any(),
            approachParameters = any(),
            options = any(),
        )
  }

  @Test
  fun `when compiling with incremental mode, incremental configuration is set`() {
    jvmCompilationConfigurationFactory.create(createFakeIncrementalKotlincMode())

    verify(compilationService).makeJvmCompilationConfiguration()
    verify(jvmCompilationConfiguration)
        .useIncrementalCompilation(
            workingDirectory = any(),
            sourcesChanges = any(),
            approachParameters = any(),
            options = any(),
        )
  }

  @Test
  fun `when compiling with incremental mode, workingDirectory is set to incrementalStateDir`() {
    val fakeIncrementalKotlincMode = createFakeIncrementalKotlincMode()

    jvmCompilationConfigurationFactory.create(fakeIncrementalKotlincMode)

    verify(jvmCompilationConfiguration)
        .useIncrementalCompilation(
            workingDirectory = eq(fakeIncrementalKotlincMode.kotlicWorkingDir.toFile()),
            sourcesChanges = any(),
            approachParameters = any(),
            options = any(),
        )
  }

  @Test
  fun `when compiling with incremental mode, sources changes are requested to be computed by the compiler`() {
    jvmCompilationConfigurationFactory.create(createFakeIncrementalKotlincMode())

    verify(jvmCompilationConfiguration)
        .useIncrementalCompilation(
            workingDirectory = any(),
            sourcesChanges = eq(SourcesChanges.ToBeCalculated),
            approachParameters = any(),
            options = any(),
        )
  }

  @Test
  fun `when compiling with incremental mode, classpath-based approach parameters are populated`() {
    val fakeIncrementalKotlincMode = createFakeIncrementalKotlincMode()

    jvmCompilationConfigurationFactory.create(createFakeIncrementalKotlincMode())

    val expectedClasspathSnapshotBasedIncrementalCompilationApproachParameters = createClasspathSnapshotBasedIncrementalCompilationApproachParameters(
        fakeIncrementalKotlincMode,
    )
    verify(jvmCompilationConfiguration)
        .useIncrementalCompilation(
            workingDirectory = any(),
            sourcesChanges = any(),
            approachParameters =
                argThat<ClasspathSnapshotBasedIncrementalCompilationApproachParameters> { parameters
                  ->
                  parameters.newClasspathSnapshotFiles ==
                      expectedClasspathSnapshotBasedIncrementalCompilationApproachParameters
                          .newClasspathSnapshotFiles &&
                      parameters.shrunkClasspathSnapshot ==
                          expectedClasspathSnapshotBasedIncrementalCompilationApproachParameters
                              .shrunkClasspathSnapshot
                },
            options = any(),
        )
  }

  @Test
  fun `when compiling with incremental mode, classpath-based compilation configuration is populated`() {
    val fakeIncrementalKotlincMode = createFakeIncrementalKotlincMode()

    jvmCompilationConfigurationFactory.create(createFakeIncrementalKotlincMode())

    verify(jvmCompilationConfiguration)
        .useIncrementalCompilation(
            workingDirectory = any(),
            sourcesChanges = any(),
            approachParameters = any(),
            options = eq(classpathSnapshotBasedIncrementalJvmCompilationConfiguration),
        )
    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration)
        .setRootProjectDir(fakeIncrementalKotlincMode.rootProjectDir.toFile())
    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration)
        .setBuildDir(fakeIncrementalKotlincMode.buildDir.toFile())
  }

  @Test
  fun `when there are no classpath changes, compiler is assured about no classpath changes`() {
    jvmCompilationConfigurationFactory.create(
        createFakeIncrementalKotlincMode(ClasspathChanges.NoChanges(ImmutableList.of())),
    )

    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration)
        .assureNoClasspathSnapshotsChanges(true)
    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration, never())
        .forceNonIncrementalMode(true)
  }

  @Test
  fun `when classpath changes can not be detected, non-incremental mode is forced`() {
    jvmCompilationConfigurationFactory.create(
        createFakeIncrementalKotlincMode(ClasspathChanges.Unknown),
    )

    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration)
        .forceNonIncrementalMode(true)
    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration, never())
        .assureNoClasspathSnapshotsChanges(true)
  }

  @Test
  fun `when requires rebuild, non-incremental mode is forced`() {
    jvmCompilationConfigurationFactory.create(
        createFakeIncrementalKotlincMode(rebuildReason = mock()),
    )

    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration)
        .forceNonIncrementalMode(true)
  }

  @Test
  fun `when rebuild is not required, non-incremental mode is not forced`() {
    jvmCompilationConfigurationFactory.create(
        createFakeIncrementalKotlincMode(rebuildReason = null),
    )

    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration, never())
        .forceNonIncrementalMode(true)
  }

  @Test
  fun `when classpath has additions or modifications only, non-incremental mode is not forced`() {
    jvmCompilationConfigurationFactory.create(
        createFakeIncrementalKotlincMode(
            classpathChanges =
                ClasspathChanges.ToBeComputedByIncrementalCompiler(ImmutableList.of()),
        ),
    )

    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration, never())
        .forceNonIncrementalMode(true)
    verify(classpathSnapshotBasedIncrementalJvmCompilationConfiguration, never())
        .assureNoClasspathSnapshotsChanges(true)
  }

  private fun createClasspathSnapshotBasedIncrementalCompilationApproachParameters(
      mode: KotlincMode.Incremental,
  ) = ClasspathSnapshotBasedIncrementalCompilationApproachParameters(
      newClasspathSnapshotFiles = mode.classpathChanges.classpathSnapshotFiles,
      shrunkClasspathSnapshot =
          mode.kotlicWorkingDir.resolve("shrunk-classpath-snapshot.bin").toFile(),
  )

  private fun createFakeIncrementalKotlincMode(
      classpathChanges: ClasspathChanges = createFakeClasspathChanges(),
      kotlinDepFile: AbsPath? = createExistingFileMock(),
      rebuildReason: RebuildReason? = null,
  ): KotlincMode.Incremental {
    val rootProjectDir = AbsPath.get("/home/root")
    val buildDir = AbsPath.get("/home/root/buildDir")
    val incrementalStateDir = AbsPath.get("/home/root/buildDir/incrementalState")

    return KotlincMode.Incremental(
        rootProjectDir,
        buildDir,
        incrementalStateDir,
        KotlinSourceChanges.ToBeCalculated,
        classpathChanges,
        kotlinDepFile,
        rebuildReason,
    )
  }

  private fun createFakeClasspathChanges(): ClasspathChanges =
      ClasspathChanges.NoChanges(ImmutableList.of(File("/home/root/buildDir/classpath.bin")))

  private fun createExistingFileMock(): AbsPath {
    val file = mock<File>().apply { whenever(this.exists()).thenReturn(true) }
    return mock<AbsPath>().apply { whenever(this.toFile()).thenReturn(file) }
  }

  private fun createNonExistingFileMock(): AbsPath {
    val file = mock<File>().apply { whenever(this.exists()).thenReturn(false) }
    return mock<AbsPath>().apply { whenever(this.toFile()).thenReturn(file) }
  }
}
