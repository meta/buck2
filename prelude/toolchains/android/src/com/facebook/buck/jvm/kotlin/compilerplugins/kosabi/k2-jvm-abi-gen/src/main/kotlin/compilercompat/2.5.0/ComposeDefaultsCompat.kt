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

/**
 * Whether this compiler's Compose plugin retypes a composable's defaulted parameters only when the
 * function takes the `$default` mask. From 2.5 an override of a function with defaults keeps its
 * parameter types and gets no value-class stub (ComposerParamTransformer guards the retyping with
 * `requiresDefaultParameter()`); 2.3 and 2.4 retype every parameter with a default, own or
 * inherited.
 */
internal const val COMPOSE_RETYPES_ONLY_WITH_DEFAULT_MASK = true
