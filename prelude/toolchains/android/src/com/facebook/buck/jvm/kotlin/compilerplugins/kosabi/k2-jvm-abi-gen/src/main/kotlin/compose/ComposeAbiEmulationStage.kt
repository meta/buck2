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

import com.facebook.buck.jvm.kotlin.compilerplugins.common.isStub
import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.com.intellij.psi.PsiElement
import org.jetbrains.kotlin.psi.KtFile

/**
 * Pipeline stage for Compose ABI emulation.
 *
 * Creates the [ComposeAbiEmulationExtension] which is registered as an [IrGenerationExtension]
 * during FIR-to-IR conversion. The extension runs before the IR sanitizer so that any
 * Compose-emulated declarations are present when non-ABI content is stripped.
 */
internal class ComposeAbiEmulationStage : AbiGenStage {
  override val name = "ComposeAbiEmulation"

  private var targetAnnotations: Map<PsiElement, ComposableTargetAnnotation> = emptyMap()

  /**
   * Infers the applier annotations the Compose backend writes. Runs on the resolved FIR, before any
   * FIR cleanup rewrites function bodies. A failure leaves the stub without them, as before, and is
   * recorded in [repairLog].
   */
  fun inferComposableTargets(
      analysisResults: FrontendOutputCompat,
      sourceFiles: List<KtFile>,
      repairLog: AbiGenRepairLog,
  ) {
    val stubPaths =
        sourceFiles.filter { it.isStub() }.map { it.viewProvider.virtualFile.path }.toSet()
    if (!COMPOSE_APPLIER_INFERENCE_SUPPORTED) return
    val output = analysisResults.outputs.lastOrNull() ?: return
    val files = analysisResults.outputs.flatMap { module -> module.fir }
    targetAnnotations =
        try {
          // The backend makes some links only depending on what it has inferred so far, which can
          // depend on code that does not resolve here. One run makes every such link, the other
          // none; an annotation is written only when both agree.
          val linked =
              ComposableTargetInference(output.session, stubPaths).also { it.analyze(files) }
          val unlinked = ComposableTargetInference(
              output.session,
              stubPaths,
              linkThroughCallTypes = false,
              nonCapturing = linked.captureUncertainLambdas(),
          )
              .also { it.analyze(files) }
          val decided =
              linked.annotations.filter { (key, annotation) ->
                unlinked.annotations[key] == annotation
              }
          // Whatever the runs agree to emit, the declarations neither could compute are recorded
          // for the applicability rule, the codemod and the driver.
          (linked.notComputed + unlinked.notComputed)
              .distinct()
              .forEach(repairLog::recordUncomputedApplier)
          // Declarations the runs computed differently get no annotation from `decided`; without
          // an entry they would be discovered only by a failing compare.
          linked.disagreementEntries(unlinked).forEach(repairLog::recordUncomputedApplier)
          decided
        } catch (e: Exception) {
          repairLog.recordFailedRepair("Compose applier inference", e.toString())
          emptyMap()
        }
  }

  /** Create the IR generation extension to be registered during FIR-to-IR conversion. */
  fun createExtension(): IrGenerationExtension {
    return ComposeAbiEmulationExtension(targetAnnotations)
  }
}
