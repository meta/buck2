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

import org.jetbrains.kotlin.backend.common.extensions.IrPluginContext
import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.Modality
import org.jetbrains.kotlin.ir.IrElement
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrDeclarationOrigin
import org.jetbrains.kotlin.ir.declarations.IrField
import org.jetbrains.kotlin.ir.declarations.IrModuleFragment
import org.jetbrains.kotlin.ir.declarations.IrProperty
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.symbols.impl.IrFieldSymbolImpl
import org.jetbrains.kotlin.ir.types.starProjectedType
import org.jetbrains.kotlin.ir.types.typeWith
import org.jetbrains.kotlin.ir.util.companionObject
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.ir.util.isInterface
import org.jetbrains.kotlin.ir.visitors.IrVisitorVoid
import org.jetbrains.kotlin.ir.visitors.acceptChildrenVoid
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

internal class ParcelableCreatorEmulator(private val pluginContext: IrPluginContext) {

  fun transform(moduleFragment: IrModuleFragment) {
    val creatorClass = pluginContext.referenceClass(PARCELABLE_CREATOR_CLASS_ID) ?: return
    moduleFragment.acceptChildrenVoid(
        object : IrVisitorVoid() {
          override fun visitElement(element: IrElement) {
            element.acceptChildrenVoid(this)
          }

          override fun visitClass(declaration: IrClass) {
            if (declaration.needsCreator()) {
              declaration.addCreatorField(creatorClass.owner)
            }
            declaration.acceptChildrenVoid(this)
          }
        },
    )
  }

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.needsCreator(): Boolean {
    if (!hasAnnotation(PARCELIZE_FQ_NAME)) return false
    if (isInterface || modality == Modality.ABSTRACT || modality == Modality.SEALED) return false
    return !declaresCreator() && companionObject()?.declaresCreator() != true
  }

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.declaresCreator(): Boolean = declarations.any {
    when (it) {
      is IrField -> it.name == CREATOR_NAME && (isCompanion || it.isStatic)
      is IrProperty -> isCompanion && it.name == CREATOR_NAME
      is IrClass -> it.name == CREATOR_NAME
      else -> false
    }
  }

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.addCreatorField(creatorClass: IrClass) {
    val field =
        pluginContext.irFactory.createField(
            startOffset = -1,
            endOffset = -1,
            origin = IrDeclarationOrigin.DEFINED,
            name = CREATOR_NAME,
            type = creatorClass.symbol.typeWith(symbol.starProjectedType),
            visibility = DescriptorVisibilities.PUBLIC,
            symbol = IrFieldSymbolImpl(),
            isFinal = true,
            isExternal = false,
            isStatic = true,
        )
    field.parent = this
    declarations.add(field)
  }

  private companion object {
    val CREATOR_NAME = Name.identifier("CREATOR")
    val PARCELIZE_FQ_NAME = FqName("kotlinx.parcelize.Parcelize")
    val PARCELABLE_CREATOR_CLASS_ID =
        ClassId(FqName("android.os"), FqName("Parcelable.Creator"), isLocal = false)
  }
}
