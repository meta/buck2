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

import org.jetbrains.kotlin.fir.FirModuleData
import org.jetbrains.kotlin.fir.FirSession
import org.jetbrains.kotlin.fir.declarations.FirDeclarationOrigin
import org.jetbrains.kotlin.fir.moduleData
import org.jetbrains.kotlin.fir.symbols.FirBasedSymbol

fun isSameModuleSource(origin: FirDeclarationOrigin): Boolean =
    origin is FirDeclarationOrigin.Source || isSameModuleJava(origin)

/**
 * Whether a declaration belongs to the target being compiled: its Kotlin or Java sources, or, in an
 * incremental round, the outputs of its unchanged files. The compiler loads those as `Precompiled`,
 * an origin it uses only for the module's own incremental-compilation binaries.
 *
 * Use it where a full compilation treats every source declaration of the target alike. Facts that
 * only a source declaration carries, such as how its type is spelled, stay [isSameModuleSource]: an
 * incremental round cannot see them for an unchanged file.
 */
fun isOwnDeclaration(origin: FirDeclarationOrigin): Boolean =
    isSameModuleSource(origin) || origin is FirDeclarationOrigin.Precompiled

/**
 * Whether [symbol] belongs to the module of [ownModuleData]: [isOwnDeclaration] by origin, or a
 * declaration the compiler loaded with that module's data. An incremental round loads the unchanged
 * files of the target that way, classes and top-level functions with the origin `Precompiled`, but
 * typealiases and top-level properties with `Library`; declarations of dependencies carry their own
 * module's data.
 */
fun isOwnDeclaration(symbol: FirBasedSymbol<*>, ownModuleData: FirModuleData): Boolean =
    isOwnDeclaration(symbol.origin) ||
        (symbol.origin is FirDeclarationOrigin.Library && symbol.moduleData == ownModuleData)

/** [isOwnDeclaration] for the module that [session] compiles. */
fun isOwnDeclaration(symbol: FirBasedSymbol<*>, session: FirSession): Boolean =
    isOwnDeclaration(symbol, session.moduleData)

// Same-compilation Java sources. Library excludes classpath bytecode, which still needs an
// explicit source-only-ABI edge.
fun isSameModuleJava(origin: FirDeclarationOrigin): Boolean =
    origin is FirDeclarationOrigin.Java && origin !is FirDeclarationOrigin.Java.Library
