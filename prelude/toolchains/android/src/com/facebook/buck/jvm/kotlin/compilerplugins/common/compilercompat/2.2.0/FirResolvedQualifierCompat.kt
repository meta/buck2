/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

package com.facebook.buck.jvm.kotlin.compilerplugins.common

import org.jetbrains.kotlin.fir.expressions.FirResolvedQualifier
import org.jetbrains.kotlin.fir.symbols.impl.FirClassLikeSymbol

/**
 * Kotlin 2.4 renamed `FirResolvedQualifier.symbol` to `qualifierSymbol`; this is that name before
 * 2.4.
 */
internal val FirResolvedQualifier.qualifierSymbol: FirClassLikeSymbol<*>?
  get() = symbol
