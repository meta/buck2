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

package com.facebook

import org.jetbrains.kotlin.ir.declarations.IrFunction
import org.jetbrains.kotlin.ir.declarations.IrParameterKind
import org.jetbrains.kotlin.ir.declarations.IrSymbolOwner
import org.jetbrains.kotlin.ir.declarations.IrValueParameter
import org.jetbrains.kotlin.ir.expressions.IrExpression
import org.jetbrains.kotlin.ir.expressions.IrMemberAccessExpression
import org.jetbrains.kotlin.ir.symbols.IrSymbol
import org.jetbrains.kotlin.ir.symbols.UnsafeDuringIrConstructionAPI
import org.jetbrains.kotlin.ir.symbols.impl.IrFakeOverrideSymbolBase

/**
 * The pre-2.4 `IrFunction.valueParameters`: context parameters then regular parameters, without the
 * dispatch or extension receiver. `parameters` holds every kind, in the order dispatch receiver,
 * context, extension receiver, regular.
 */
val IrFunction.compatValueParameters: List<IrValueParameter>
  get() = parameters.filter {
    it.kind == IrParameterKind.Context || it.kind == IrParameterKind.Regular
  }

/**
 * [index] is a position in [compatValueParameters]; `arguments` also holds the receivers, so the
 * index is mapped through the callee's parameters.
 */
fun IrMemberAccessExpression<*>.getValueArgumentCompat(index: Int): IrExpression? =
    arguments[argumentIndexForOldValueArgument(index)]

/** [index] is a position in [compatValueParameters]. */
fun IrMemberAccessExpression<*>.putValueArgumentCompat(index: Int, valueArgument: IrExpression?) {
  arguments[argumentIndexForOldValueArgument(index)] = valueArgument
}

/** The pre-2.4 `valueArgumentsCount`: context and regular arguments, receivers excluded. */
val IrMemberAccessExpression<*>.valueArgumentsCountCompat: Int
  get() =
      (symbol.safeOwner() as? IrFunction)?.compatValueParameters?.size
          ?: error("valueArgumentsCountCompat used on a non-function access")

private fun IrMemberAccessExpression<*>.argumentIndexForOldValueArgument(index: Int): Int {
  val callee =
      symbol.safeOwner() as? IrFunction
          ?: error("Old value-argument index used on a non-function access")
  return callee.compatValueParameters[index].indexInParameters
}

/**
 * The owner of [this], or null when it is unbound. An unbound fake override's `owner` throws, but
 * it shares its original's signature, so it resolves through the original symbol.
 */
@OptIn(UnsafeDuringIrConstructionAPI::class)
private fun IrSymbol.safeOwner(): IrSymbolOwner? =
    try {
      var current: IrSymbol = this
      val seen = HashSet<IrSymbol>()
      while (current is IrFakeOverrideSymbolBase<*, *, *> && seen.add(current)) {
        current = current.originalSymbol
      }
      current.owner
    } catch (_: IllegalStateException) {
      null
    }
