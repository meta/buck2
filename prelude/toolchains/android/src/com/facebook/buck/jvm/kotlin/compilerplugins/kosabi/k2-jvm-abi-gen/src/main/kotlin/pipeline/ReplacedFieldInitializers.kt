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

import org.jetbrains.kotlin.descriptors.DescriptorVisibilities
import org.jetbrains.kotlin.descriptors.DescriptorVisibility
import org.jetbrains.kotlin.ir.declarations.IrClass
import org.jetbrains.kotlin.ir.declarations.IrField
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.util.fileOrNull
import org.jetbrains.kotlin.ir.util.kotlinFqName

/**
 * Records that the IR stripper replaced or discarded [field]'s initializer.
 *
 * For a `const val` the initializer is the `ConstantValue` attribute consumers inline, so
 * [ValidationStage] needs to know whether the field is const, whether a consumer can see it, and
 * where it is declared.
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
internal fun AbiGenRepairLog.recordReplacedFieldInitializer(field: IrField, detail: String) {
  val property = field.correspondingPropertySymbol?.owner
  val ownerPrefix = runCatching { field.parent.kotlinFqName.asString() }.getOrDefault("")
  recordReplacedFieldInitializer(
      if (ownerPrefix.isEmpty()) field.name.asString() else "$ownerPrefix.${field.name}",
      detail,
      isConst = property?.isConst == true,
      consumerVisible = isConsumerVisible(property?.visibility ?: field.visibility, field),
      position = field.sourcePosition(),
  )
}

/**
 * Whether another module can read [field] in the source-only ABI: neither the property nor any
 * enclosing class, nested or top-level, is private or local. Internal declarations count: Kosabi
 * keeps them in the ABI for friend modules, which inline their constants.
 */
private fun isConsumerVisible(visibility: DescriptorVisibility, field: IrField): Boolean {
  if (visibility.isHidden()) return false
  var owner = field.parent as? IrClass
  while (owner != null) {
    if (owner.visibility.isHidden()) return false
    owner = owner.parent as? IrClass
  }
  return true
}

private fun DescriptorVisibility.isHidden(): Boolean =
    this == DescriptorVisibilities.PRIVATE ||
        this == DescriptorVisibilities.PRIVATE_TO_THIS ||
        this == DescriptorVisibilities.LOCAL

private fun IrField.sourcePosition(): SourcePosition? {
  val fileEntry = fileOrNull?.fileEntry ?: return null
  if (startOffset < 0) return null
  return SourcePosition(
      fileEntry.name,
      fileEntry.getLineNumber(startOffset) + 1,
      fileEntry.getColumnNumber(startOffset) + 1,
  )
}
