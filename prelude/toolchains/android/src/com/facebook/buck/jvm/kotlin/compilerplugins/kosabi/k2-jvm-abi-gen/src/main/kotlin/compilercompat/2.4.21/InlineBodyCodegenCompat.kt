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

import org.jetbrains.kotlin.cli.common.messages.MessageCollector
import org.jetbrains.kotlin.ir.declarations.IrSimpleFunction

// Every method is generated from its IR body, and the IR stripper has replaced every body except
// the
// kept ones, so kept bodies need no code generation hook.
internal fun generateCodeFromIrWithInlineBodiesCompat(
    input: ModuleCompilerIrBackendInputCompat,
    environment: ModuleCompilerEnvironmentCompat,
    keptInlineBodies: Set<IrSimpleFunction>,
    messageCollector: MessageCollector,
): ModuleCompilerOutputCompat = generateCodeFromIrCompat(input, environment)
