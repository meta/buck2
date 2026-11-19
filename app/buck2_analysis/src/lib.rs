/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(box_patterns)]
#![feature(try_blocks)]

pub mod analysis;
pub mod attrs;

pub fn init_late_bindings() {
    attrs::resolve::configured_attr::init_configured_attr_to_value();
    analysis::calculation::init_rule_analysis_calculation();
}
