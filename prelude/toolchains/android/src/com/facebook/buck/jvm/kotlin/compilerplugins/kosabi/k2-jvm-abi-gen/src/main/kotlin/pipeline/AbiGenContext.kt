/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

@file:SuppressWarnings("PackageLocationMismatch")

package com.facebook

import java.io.File
import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.config.CompilerConfiguration
import org.jetbrains.kotlin.psi.KtFile

/**
 * Context passed through the ABI generation pipeline stages.
 *
 * Populated progressively as the pipeline advances through phases:
 * - firResult is set after FIR frontend analysis
 * - irBackendInput is set after FIR-to-IR conversion
 */
class AbiGenContext(
    val outputDir: File,
    val messageCollector: MessageCollector,
    val configuration: CompilerConfiguration,
    val sourceFiles: List<KtFile>,
    val firResult: FirResultCompat,
) {
  /** Set after FIR-to-IR conversion. */
  var irBackendInput: ModuleCompilerIrBackendInputCompat? = null
}
