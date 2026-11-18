/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:Suppress("PackageLocationMismatch")

package com.facebook.kotlin.compilerplugins.kosabi.stubsgen.cli

import com.facebook.kotlin.compilerplugins.common.createFakeKtFile
import java.io.Closeable
import java.io.File
import org.jetbrains.kotlin.cli.jvm.compiler.setupIdeaStandaloneExecution
import org.jetbrains.kotlin.com.intellij.core.CoreApplicationEnvironment
import org.jetbrains.kotlin.com.intellij.core.CoreProjectEnvironment
import org.jetbrains.kotlin.com.intellij.lang.LanguageParserDefinitions
import org.jetbrains.kotlin.com.intellij.openapi.Disposable
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.com.intellij.openapi.util.Disposer
import org.jetbrains.kotlin.com.intellij.psi.PsiManager
import org.jetbrains.kotlin.idea.KotlinLanguage
import org.jetbrains.kotlin.parsing.KotlinParserDefinition
import org.jetbrains.kotlin.psi.KtFile

// Parse-only PSI env: avoids createForProduction's extension points + classpath index.
class StubsGenParseOnlyEnvironment : Closeable {

  private val disposable: Disposable = Disposer.newDisposable("StubsGenParseOnlyEnvironment")

  init {
    setupIdeaStandaloneProperties()
  }

  private val applicationEnvironment: CoreApplicationEnvironment =
      CoreApplicationEnvironment(disposable)

  private val projectEnvironment: CoreProjectEnvironment =
      CoreProjectEnvironment(disposable, applicationEnvironment)

  val project: Project = projectEnvironment.project

  init {
    // Guard against double-registration (a test JVM may already have a Kotlin parser).
    if (LanguageParserDefinitions.INSTANCE.forLanguage(KotlinLanguage.INSTANCE) == null) {
      applicationEnvironment.registerParserDefinition(KotlinParserDefinition())
    }
  }

  fun parse(sourceFiles: List<File>): List<KtFile> = sourceFiles.map { file ->
    // Strip leading BOM to match the in-kotlinc LoadTextUtil (readText does not).
    val content = file.readText().removePrefix("﻿")
    createFakeKtFile(
        fileManager = PsiManager.getInstance(project),
        virtualFilePath = file.path,
        name = file.name,
        content = content,
    )
  }

  override fun close() {
    Disposer.dispose(disposable)
  }

  private companion object {
    private fun setupIdeaStandaloneProperties() {
      // The compiler's own standalone setup; createForProduction runs it, this env bypasses that.
      setupIdeaStandaloneExecution()
      setIfAbsent("idea.io.use.nio2", "true")
      // idea.home.path must point at an existing dir or PathManager throws; use the JVM temp dir.
      setIfAbsent("idea.home.path", System.getProperty("java.io.tmpdir"))
    }

    private fun setIfAbsent(key: String, value: String) {
      if (System.getProperty(key).isNullOrEmpty()) {
        System.setProperty(key, value)
      }
    }
  }
}
