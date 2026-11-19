/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.generators

import com.facebook.buck.jvm.kotlin.compilerplugins.kosabi.common.stub.render.StubBytecodeRender
import com.facebook.buck.jvm.kotlin.compilerplugins.kosabi.common.stub.render.StubBytecodeRender.supportBytecode
import com.facebook.kotlin.compilerplugins.kosabi.common.FullTypeQualifier
import com.facebook.kotlin.compilerplugins.kosabi.common.Logger
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.KStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.model.generateFakeKtFile
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.render.RenderedKStub
import com.facebook.kotlin.compilerplugins.kosabi.common.stub.render.render
import com.facebook.kotlin.compilerplugins.kosabi.stubsgen.util.JavaSourceInfo
import java.io.File
import java.nio.file.Files
import kotlin.collections.forEach
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.psi.KtFile

open class StubsGenAPI(
    private val stubsDumpDir: File? = null,
    private val stubsClassOutputDir: File? = null,
    private val classPaths: List<File>,
) {

  fun generateStubs(
      knownSources: Collection<KtFile>,
      configuration: CompilerConfiguration,
      project: Project,
      javaSources: Collection<JavaSourceInfo> = emptyList(),
  ): Collection<KtFile> {

    // Star imports never reach stub creation (JavaSourceInfo keeps them out of `imports`):
    // like their Kotlin counterparts, they are ignored here and gated at onboarding time
    // (kosabi.py flags them), so existing targets keep building unchanged.
    Logger.log("[Source files] ${knownSources.joinToString(", ") { it.name }}")

    // Why we have two rounds of `generateStubs` here?
    //
    // 1. [additionalSourcesGenerators] creates a set of additional sources ([KStub]s that we treat
    // as real files)
    // An additional source file is usually a complicated stub that is included in ABI.
    //
    // 2. If we run [additionalSourcesGenerators] as a part of [CustomStubsGenerator]
    // we will need to run all the stubs that generate stub classes structure (primitive, ctor, ...)
    // for types that we introduced during [additionalSourcesGenerators]
    //   2a. or alternatively create all these stubs (but it's a lot of work)
    //
    // 3. So we call [additionalSourcesGenerators] with it's own [generationContext]
    //   3a. Than dump context creating additional source files (similar to what AP usually does)
    //   3b. Than run the second round  with extended [knownSources]
    Logger.log("[Source files] ${knownSources.joinToString(", ") { it.name }}")
    val additionalSrcStubsContext = GenerationContext(knownSources, classPaths, lightweight = true)
    val additionalSrcStubs =
        generateStubs(configureAdditionalSourcesGenerators(), additionalSrcStubsContext)
    val additionalRenderedSrcStubs: List<RenderedKStub> = additionalSrcStubs.map {
      RenderedKStub(it, it.render())
    }
    val additionalSrcs = additionalRenderedSrcStubs.map { generateFakeKtFile(project, it) }

    val stubsGenerators: List<StubsGenerator> = listOf(
        PrimitiveStubsGenerator(getPostfixToSkipForPrimitiveTypesGeneration()),
        InnerClassStubsGenerator(),
        FullQualifiedClassStubsGenerator(),
        SamePackageClassStubsGenerator(),
        CtorStubsGenerator(),
        InterfaceStubsGenerator(),
        GenericStubsGenerator(),
        AnnotationStubsGenerator(),
        ClassLevelFunctionStubsGenerator(),
    )
    val context = GenerationContext(
        additionalSrcs + knownSources,
        classPaths,
        getKnownKspGeneratedTypes(),
        lightweight = false,
        javaSources,
    )
    val stubs = generateStubs(stubsGenerators + configureExtraGenerators(), context)
    val enableBytecodeStub = stubsClassOutputDir != null
    val renderedStubs: List<RenderedKStub> =
        stubs
            .filter { stub -> !enableBytecodeStub || !stub.supportBytecode() }
            .map { RenderedKStub(it, it.render()) }
    val stubFiles = renderedStubs.map { generateFakeKtFile(project, it) }

    if (stubsDumpDir != null) {
      val stubDir = stubsDumpDir
      check(stubDir.exists() || stubDir.mkdirs()) {
        "Could not generate package directory: $stubDir"
      }

      (additionalSrcs + stubFiles).forEach { file ->
        File(stubsDumpDir, file.name).writeText(file.text)
      }
    }

    if (stubsClassOutputDir != null) {
      val stubClassDir = stubsClassOutputDir
      check(stubClassDir.exists() || stubClassDir.mkdirs()) {
        "Could not generate package directory: $stubClassDir"
      }
      stubs
          .filter { stub -> stub.supportBytecode() }
          .forEach { StubBytecodeRender.renderKStubBytecode(it, stubsClassOutputDir) }
    }

    return additionalSrcs + stubFiles
  }

  open fun configureExtraGenerators(): List<StubsGenerator> {
    return emptyList()
  }

  open fun configureAdditionalSourcesGenerators(): List<StubsGenerator> {
    return emptyList()
  }

  open fun getKnownKspGeneratedTypes(): Set<FullTypeQualifier> {
    return emptySet()
  }

  open fun getPostfixToSkipForPrimitiveTypesGeneration(): String? {
    return null
  }

  fun writeStubs(dir: File, stubs: Collection<RenderedKStub>) {
    stubs.forEach { renderedStub ->
      val relativeDir = renderedStub.stub.pkg?.replace(".", "/")
      val targetDir = if (relativeDir.isNullOrEmpty()) dir else dir.resolve(relativeDir)
      Files.createDirectories(targetDir.toPath())
      val filename = "${renderedStub.stub.name}.kt"
      File(targetDir, filename).writeText(renderedStub.render)
    }
  }

  private fun generateStubs(
      generators: List<StubsGenerator>,
      context: GenerationContext,
  ): Collection<KStub> {
    generators.forEach { generator ->
      Logger.log("[${generator.javaClass.simpleName}]")
      generator.generateStubs(context)
    }
    return context.stubsContainer.all()
  }
}
