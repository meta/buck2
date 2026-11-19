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

import org.jetbrains.kotlin.fir.declarations.FirNamedFunction
import org.jetbrains.kotlin.fir.visitors.FirDefaultVisitorVoid

// In Kotlin 2.2 the visitor callback is visitSimpleFunction(FirSimpleFunction); in 2.3 it was
// renamed to visitNamedFunction(FirNamedFunction). Bridge both to visitNamedFunctionCompat.
// FirNamedFunctionCompat is declared in FrontendPipelineCompat.kt (same package).
abstract class FirDefaultVisitorVoidCompat : FirDefaultVisitorVoid() {
  abstract fun visitNamedFunctionCompat(namedFunction: FirNamedFunctionCompat)

  override fun visitNamedFunction(namedFunction: FirNamedFunction) {
    visitNamedFunctionCompat(namedFunction)
  }
}
