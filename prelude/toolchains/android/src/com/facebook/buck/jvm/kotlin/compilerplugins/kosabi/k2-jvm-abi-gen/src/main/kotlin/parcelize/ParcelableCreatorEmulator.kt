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
import org.jetbrains.kotlin.descriptors.ClassKind
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
import org.jetbrains.kotlin.ir.types.classOrNull
import org.jetbrains.kotlin.ir.types.starProjectedType
import org.jetbrains.kotlin.ir.types.typeWith
import org.jetbrains.kotlin.ir.util.companionObject
import org.jetbrains.kotlin.ir.util.hasAnnotation
import org.jetbrains.kotlin.ir.visitors.IrVisitorVoid
import org.jetbrains.kotlin.ir.visitors.acceptChildrenVoid
import org.jetbrains.kotlin.name.ClassId
import org.jetbrains.kotlin.name.FqName
import org.jetbrains.kotlin.name.Name

internal class ParcelableCreatorEmulator(
    private val pluginContext: IrPluginContext,
    private val parcelizeAnnotations: List<FqName>,
    private val polymorphicSealedAnnotations: List<FqName>,
) {

  fun transform(moduleFragment: IrModuleFragment) {
    val candidates = mutableListOf<IrClass>()
    moduleFragment.acceptChildrenVoid(
        object : IrVisitorVoid() {
          override fun visitElement(element: IrElement) {
            element.acceptChildrenVoid(this)
          }

          override fun visitClass(declaration: IrClass) {
            if (declaration.needsCreator()) candidates.add(declaration)
            declaration.acceptChildrenVoid(this)
          }
        },
    )
    if (candidates.isEmpty()) return
    val creatorClass =
        checkNotNull(pluginContext.referenceClass(PARCELABLE_CREATOR_CLASS_ID)) {
          "Cannot export Parcelize CREATOR: android.os.Parcelable.Creator is unavailable"
        }
    candidates.forEach { it.addCreatorField(creatorClass.owner) }
  }

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.needsCreator(): Boolean {
    val eligible =
        if (modality == Modality.SEALED) {
          isPolymorphicSealedRoot()
        } else {
          kind in PARCELIZE_CLASS_KINDS && modality != Modality.ABSTRACT && isParcelizeClass()
        }
    return eligible && !declaresCreator() && companionObject()?.declaresCreator() != true
  }

  // A backend that declares polymorphic sealed annotations creates the root's CREATOR before it
  // skips sealed classes, and also accepts a sealed interface root.
  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.isPolymorphicSealedRoot(): Boolean =
      polymorphicSealedAnnotations.any { hasAnnotation(it) } &&
          (kind == ClassKind.INTERFACE || (kind in PARCELIZE_CLASS_KINDS && isParcelizeClass()))

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.isParcelizeClass(): Boolean =
      hasParcelizeAnnotation() ||
          superTypes.any { type ->
            val parent = type.classOrNull?.owner
            parent?.modality == Modality.SEALED && parent.hasParcelizeAnnotation()
          }

  @OptIn(UnsafeDuringIrConstructionAPI::class)
  private fun IrClass.hasParcelizeAnnotation(): Boolean = parcelizeAnnotations.any {
    hasAnnotation(it)
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
    val PARCELIZE_CLASS_KINDS = setOf(ClassKind.CLASS, ClassKind.OBJECT, ClassKind.ENUM_CLASS)
    val PARCELABLE_CREATOR_CLASS_ID =
        ClassId(FqName("android.os"), FqName("Parcelable.Creator"), isLocal = false)
  }
}
