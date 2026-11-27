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
@file:OptIn(org.jetbrains.kotlin.DeprecatedForRemovalCompilerApi::class)

package com.facebook

import org.jetbrains.kotlin.ir.declarations.IrFunction
import org.jetbrains.kotlin.ir.declarations.IrValueParameter
import org.jetbrains.kotlin.ir.expressions.IrExpression
import org.jetbrains.kotlin.ir.expressions.IrMemberAccessExpression

/** Context and regular parameters, without the dispatch or extension receiver. */
val IrFunction.compatValueParameters: List<IrValueParameter>
  get() = valueParameters

/** [index] is a position in [compatValueParameters]. */
fun IrMemberAccessExpression<*>.getValueArgumentCompat(index: Int): IrExpression? =
    getValueArgument(index)

/** [index] is a position in [compatValueParameters]. */
fun IrMemberAccessExpression<*>.putValueArgumentCompat(index: Int, valueArgument: IrExpression?) {
  putValueArgument(index, valueArgument)
}

/** The number of context and regular arguments, receivers excluded. */
val IrMemberAccessExpression<*>.valueArgumentsCountCompat: Int
  get() = valueArgumentsCount
