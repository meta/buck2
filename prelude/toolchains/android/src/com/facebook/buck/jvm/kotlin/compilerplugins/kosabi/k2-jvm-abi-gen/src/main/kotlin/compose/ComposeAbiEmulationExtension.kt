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

import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.com.intellij.psi.PsiElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment

/**
 * IR generation extension for Compose ABI emulation.
 *
 * When enabled, this extension runs before the IR sanitizer during FIR-to-IR conversion. It
 * emulates the Compose compiler plugin's effects on the ABI surface:
 *
 * 1. **ClassStabilityTransformer** — adds $stable field and @StabilityInferred annotation to
 *    eligible classes (the P1 ClassStabilityTransformer equivalent)
 * 2. **ComposableDefaultParamEmulator** — moves the defaults of a virtual @Composable function into
 *    a `ComposeDefaultImpls` wrapper (the ComposableDefaultParamLowering equivalent)
 * 3. **ComposerParamInjector** — adds $composer, $changed, $default parameters to @Composable
 *    functions (the P4 ComposerParamTransformer equivalent)
 * 4. **ComposableTypeRewriter** — rewrites @Composable function types in parameters, return types,
 *    and supertypes to include the synthetic Composer and changed-bitmask type arguments
 * 5. **ComposableTargetAnnotator** — writes the applier annotations (`ComposableTarget`,
 *    `ComposableInferredTarget`, `ComposableOpenTarget`) that [ComposableTargetInference] inferred
 *    from the FIR bodies (the ComposableTargetAnnotationsTransformer equivalent)
 *
 * These transforms run before the IR sanitizer so that Compose-emulated declarations are present
 * when non-ABI content is stripped.
 */
internal class ComposeAbiEmulationExtension(
    private val composableTargets: Map<PsiElement, ComposableTargetAnnotation> = emptyMap(),
    private val declaredSuperclasses: Map<IrClass, IrClass?> = emptyMap(),
) : IrGenerationExtension {

  override fun generate(moduleFragment: IrModuleFragment, pluginContext: IrPluginContext) {
    // Pass 1: Add $stable field + @StabilityInferred to eligible classes.
    ClassStabilityTransformer(pluginContext, declaredSuperclasses).transform(moduleFragment)

    // Pass 2: Move the defaults of virtual @Composable functions into ComposeDefaultImpls wrappers.
    ComposableDefaultParamEmulator(pluginContext).transform(moduleFragment)

    // Pass 3: Inject $composer, $changed, $default params into @Composable functions.
    ComposerParamInjector(pluginContext).transform(moduleFragment)

    // Pass 4: Rewrite @Composable function types in parameter types, return types, supertypes.
    ComposableTypeRewriter(pluginContext).transform(moduleFragment)

    // Pass 5: Write the inferred applier annotations.
    ComposableTargetAnnotator(pluginContext, composableTargets).transform(moduleFragment)
  }
}
