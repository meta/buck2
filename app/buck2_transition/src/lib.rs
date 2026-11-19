/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(try_blocks)]

pub(crate) mod transition;

pub fn init_late_bindings() {
    transition::calculation_apply_transition::init_transition_calculation();
    transition::calculation_fetch_transition::init_transition_attr_provider();
    transition::starlark::init_register_transition();
}
