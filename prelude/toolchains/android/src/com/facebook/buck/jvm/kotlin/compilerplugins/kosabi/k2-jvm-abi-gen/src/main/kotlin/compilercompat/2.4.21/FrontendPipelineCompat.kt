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
@file:OptIn(
    org.jetbrains.kotlin.K1Deprecation::class,
    org.jetbrains.kotlin.psi.KtImplementationDetail::class,
)

package com.facebook

import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.com.intellij.openapi.project.Project
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.diagnostics.impl.BaseDiagnosticsCollector
import org.jetbrains.kotlin.diagnostics.impl.DiagnosticsCollectorImpl
import org.jetbrains.kotlin.extensions.CompilerConfigurationExtension
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirFile
import org.jetbrains.kotlin.fir.declarations.FirNamedFunction
import org.jetbrains.kotlin.fir.pipeline.AllModulesFrontendOutput
import org.jetbrains.kotlin.fir.pipeline.SingleModuleFrontendOutput
import org.jetbrains.kotlin.fir.resolve.ScopeSession
import org.jetbrains.kotlin.fir.visitors.FirDefaultVisitorVoid
import org.jetbrains.kotlin.psi.KtFile
import org.jetbrains.kotlin.psi.hmppModuleName
import org.jetbrains.kotlin.psi.isCommonSource

// ast-grep-ignore: common/kotlin/no-new-typealias-warn
typealias FrontendOutputCompat = AllModulesFrontendOutput

// ast-grep-ignore: common/kotlin/no-new-typealias-warn
typealias ModuleFrontendOutputCompat = SingleModuleFrontendOutput

// ast-grep-ignore: common/kotlin/no-new-typealias-warn
typealias FirNamedFunctionCompat = FirNamedFunction

fun moduleFrontendOutputCompat(
    session: FirSession,
    scopeSession: ScopeSession,
    fir: List<FirFile>,
): ModuleFrontendOutputCompat = SingleModuleFrontendOutput(session, scopeSession, fir)

fun frontendOutputCompat(outputs: List<ModuleFrontendOutputCompat>): FrontendOutputCompat =
    AllModulesFrontendOutput(outputs)

abstract class FirNamedFunctionVisitorCompat : FirDefaultVisitorVoid() {
  final override fun visitNamedFunction(namedFunction: FirNamedFunction) {
    visitNamedFunctionCompat(namedFunction)
  }

  abstract fun visitNamedFunctionCompat(function: FirNamedFunctionCompat)
}

/** Kotlin 2.4 removed `DiagnosticReporterFactory`; the default collector only collects. */
fun createPendingReporterCompat(messageCollector: MessageCollector): BaseDiagnosticsCollector =
    DiagnosticsCollectorImpl()

val ModuleCompilerOutputCompat.classFileFactoryCompat
  get() = factory

fun CompilerConfiguration.retainOutputInMemoryCompat() {
  // No-op: output is not released after writing since Kotlin 2.3.
}

/**
 * `KtFile.isCommonSource` and `KtFile.hmppModuleName` moved to `org.jetbrains.kotlin.psi` in 2.4.
 */
var KtFile.isCommonSourceCompat: Boolean?
  get() = isCommonSource
  set(value) {
    isCommonSource = value
  }

var KtFile.hmppModuleNameCompat: String?
  get() = hmppModuleName
  set(value) {
    hmppModuleName = value
  }

fun CompilerConfigurationExtension.updateFileRegistryCompat(project: Project) {
  updateFileRegistry(project)
}
