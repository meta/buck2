/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.cd.workertool.postexecutors

import com.facebook.buck.core.filesystems.AbsPath
import com.facebook.buck.jvm.java.abi.StubJar
import com.facebook.buck.testutil.TemporaryPaths
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.mockito.kotlin.mock
import org.mockito.kotlin.verify

internal class IncrementalAbiWriterTest {

  @JvmField @Rule val temporaryFolder: TemporaryPaths = TemporaryPaths(false)

  private val kotlincOutputStubJar = mock<StubJar>()
  private val libraryStubJar = mock<StubJar>()

  private lateinit var libraryJar: AbsPath
  private lateinit var jvmAbiGenWorkingDir: AbsPath
  private lateinit var jvmAbiGenJar: AbsPath
  private lateinit var abiJar: AbsPath

  @Before
  fun setUp() {
    jvmAbiGenJar = temporaryFolder.root.resolve("jvm-abi-gen.jar")
    libraryJar = temporaryFolder.newFile("library.jar")
    abiJar = temporaryFolder.newFile("abi.jar")
  }

  @Test
  fun `when jvmAbiGenOutput exists, abi jar is created`() {
    jvmAbiGenWorkingDir = temporaryFolder.newFile("jvm_abi_gen_output")
    val writer = IncrementalAbiWriter(
        kotlincOutputStubJar,
        jvmAbiGenWorkingDir,
        jvmAbiGenJar,
        libraryStubJar,
        abiJar,
    )

    writer.execute()

    verify(kotlincOutputStubJar).setExistingAbiJar(jvmAbiGenWorkingDir)
    verify(kotlincOutputStubJar).writeTo(jvmAbiGenJar)
    verify(libraryStubJar).setExistingAbiJar(jvmAbiGenJar)
    verify(libraryStubJar).writeTo(abiJar)
  }
}
