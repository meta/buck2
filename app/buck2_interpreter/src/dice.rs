/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The dice module contains the interpreter's integration with dice. This
//! module contains the extension traits that we implement for `Arc<DiceCtx>`
//! (the implementations of the traits are in the submodules).
//!
//! Several of these extension traits provide implementations of our delegate/DI
//! traits that are themselves build on dice (ex DiceInterpreterFileOps
//! implements InterpreterFileOps by basically putting DefaultInterpreterFileOps
//! onto the dice graph).

pub mod starlark_debug;
pub mod starlark_provider;
pub mod starlark_types;

#[cfg(all(test, buck_build))]
mod pagable_starlark_test;
