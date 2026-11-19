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
@file:Suppress("OPT_IN_USAGE_ERROR")

package com.facebook

import org.jetbrains.kotlin.backend.common.extensions.IrGenerationExtension
import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.util.isInterface

/**
 * Records the superclass of every class of the module before the plugin IR extensions that abi-gen
 * runs ([ABI_RELEVANT_PLUGIN_IR_EXTENSIONS]) change it.
 *
 * The compiler's Compose plugin infers stability before DataClassGenerate replaces the `Any`
 * superclass of a data class with `DataClassSuper`: the library jar extends `DataClassSuper` and
 * still writes the known-stable bit for a data class of stable members, which it could not do with
 * `DataClassSuper` combined in. abi-gen runs DataClassGenerate's extension before its Compose
 * emulation, so [ClassStabilityTransformer] combines the superclass recorded here.
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal class DeclaredSuperclassRecorder(
    private val superclasses: MutableMap<IrClass, IrClass?>,
) : IrGenerationExtension {

  override fun generate(moduleFragment: IrModuleFragment, pluginContext: IrPluginContext) {
    superclasses.clear()
    moduleFragment.accept(
        object : IrElementVisitorVoidCompat() {
          override fun visitElement(element: IrElement) {
            element.acceptChildren(this, null)
          }

          override fun visitClass(declaration: IrClass) {
            superclasses[declaration] = declaration.superclassOrNull()
            super.visitClass(declaration)
          }
        },
        null,
    )
  }
}

/** The first supertype of [this] that is not an interface (`Any` when it declares none). */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal fun IrClass.superclassOrNull(): IrClass? =
    superTypes.mapNotNull { it.classOrNull?.owner }.firstOrNull { !it.isInterface }
