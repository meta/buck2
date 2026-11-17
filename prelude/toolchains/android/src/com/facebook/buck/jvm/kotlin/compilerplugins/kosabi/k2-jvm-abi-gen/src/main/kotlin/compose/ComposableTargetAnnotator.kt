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

import org.jetbrains.kotlin.KtPsiSourceElement
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.backend.common.lower.createIrBuilder
import org.jetbrains.kotlin.com.intellij.psi.PsiElement
import org.jetbrains.kotlin.fir.backend.FirMetadataSource
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrFunction
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.expressions.impl.IrConstImpl
import org.jetbrains.kotlin.ir.symbols.IrClassSymbol
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.util.constructors
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

/**
 * Writes the applier annotations [ComposableTargetInference] inferred onto the module's IR
 * declarations, as the Compose backend does: on the declaration and as metadata-visible, so both
 * the class file and `kotlin.Metadata` carry them.
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class ComposableTargetAnnotator(
    private val pluginContext: IrPluginContext,
    private val annotations: Map<PsiElement, ComposableTargetAnnotation>,
) {
  private val targetClass by lazy { pluginContext.referenceClass(COMPOSABLE_TARGET) }
  private val openTargetClass by lazy { pluginContext.referenceClass(COMPOSABLE_OPEN_TARGET) }
  private val inferredTargetClass by lazy {
    pluginContext.referenceClass(COMPOSABLE_INFERRED_TARGET)
  }

  fun transform(moduleFragment: IrModuleFragment) {
    if (annotations.isEmpty()) return
    // The backend only infers when the runtime declares all three annotations.
    if (targetClass == null || openTargetClass == null || inferredTargetClass == null) return
    moduleFragment.accept(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            if (element is IrFunction) annotate(element)
            element.acceptChildren(this, null)
          }
        },
        null,
    )
  }

  private fun annotate(function: IrFunction) {
    val fir = (function.metadata as? FirMetadataSource.Function)?.fir ?: return
    val psi = (fir.source as? KtPsiSourceElement)?.psi ?: return
    val annotation = annotations[psi] ?: return
    val (annotationClass, argument) =
        when (annotation) {
          is ComposableTargetAnnotation.Target ->
              targetClass to
                  IrConstImpl.string(
                      -1,
                      -1,
                      pluginContext.irBuiltIns.stringType,
                      annotation.applier,
                  )
          is ComposableTargetAnnotation.OpenTarget ->
              openTargetClass to
                  IrConstImpl.int(-1, -1, pluginContext.irBuiltIns.intType, annotation.index)
          is ComposableTargetAnnotation.InferredTarget ->
              inferredTargetClass to
                  IrConstImpl.string(
                      -1,
                      -1,
                      pluginContext.irBuiltIns.stringType,
                      annotation.scheme,
                  )
        }
    val constructor = (annotationClass as IrClassSymbol).constructors.singleOrNull() ?: return
    val irAnnotation =
        pluginContext.irBuiltIns.createIrBuilder(function.symbol).run {
          irAnnotationCompat(constructor, emptyList()).apply { arguments[0] = argument }
        }
    pluginContext.metadataDeclarationRegistrar.addMetadataVisibleAnnotationsToElement(
        function,
        irAnnotation,
    )
  }

  private companion object {
    private val RUNTIME = FqName("androidx.compose.runtime")
    val COMPOSABLE_TARGET = ClassId(RUNTIME, Name.identifier("ComposableTarget"))
    val COMPOSABLE_OPEN_TARGET = ClassId(RUNTIME, Name.identifier("ComposableOpenTarget"))
    val COMPOSABLE_INFERRED_TARGET = ClassId(RUNTIME, Name.identifier("ComposableInferredTarget"))
  }
}
