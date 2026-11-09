/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.kotlin.compilerplugins.common

import org.jetbrains.kotlin.fir.declarations.FirDeclarationOrigin

fun isSameModuleSource(origin: FirDeclarationOrigin): Boolean =
    origin is FirDeclarationOrigin.Source || isSameModuleJava(origin)

// Same-compilation Java sources. Library excludes classpath bytecode, which still needs an
// explicit source-only-ABI edge.
fun isSameModuleJava(origin: FirDeclarationOrigin): Boolean =
    origin is FirDeclarationOrigin.Java && origin !is FirDeclarationOrigin.Java.Library
