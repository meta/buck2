/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod configuration;
pub mod execution;
pub mod nodes;
mod target_platform_resolution;

pub fn init_late_bindings() {
    target_platform_resolution::init_configured_target_calculation();
    execution::init_get_execution_platforms();
    configuration::init_configuration_calculation();
    nodes::init_configured_target_node_calculation();
}
