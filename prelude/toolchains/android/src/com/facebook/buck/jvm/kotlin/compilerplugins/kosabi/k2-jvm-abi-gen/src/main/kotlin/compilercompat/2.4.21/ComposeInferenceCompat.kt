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

import androidx.compose.compiler.plugins.kotlin.inference.ErrorReporter
import androidx.compose.compiler.plugins.kotlin.inference.Scheme
import org.jetbrains.kotlin.fir.expressions.FirExpression
import org.jetbrains.kotlin.fir.expressions.FirFunctionTypeConversionExpression

/** Whether abi-gen infers the Compose applier annotations with this compiler. */
internal const val COMPOSE_APPLIER_INFERENCE_SUPPORTED = true

/** The expression a function-type conversion wraps. */
internal fun functionTypeConversionArgument(expression: FirExpression): FirExpression? =
    (expression as? FirFunctionTypeConversionExpression)?.expression

internal fun Scheme.serializeCompat(): String = serialize()

internal fun <Node> silentErrorReporter(): ErrorReporter<Node> =
    object : ErrorReporter<Node> {
      override fun reportCallError(node: Node, expected: String, received: String) {}

      override fun reportParameterError(
          node: Node,
          index: Int,
          expected: String,
          received: String,
      ) {}

      override fun log(node: Node?, message: String) {}
    }
