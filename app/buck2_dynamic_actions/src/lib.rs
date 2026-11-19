/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(arbitrary_self_types)]

use std::sync::Once;

pub mod attrs;
pub mod attrs_starlark;
pub mod bxl;
pub mod calculation;
pub mod deferred;
pub mod dynamic_actions;
pub mod dynamic_actions_callable;
pub mod dynamic_actions_globals;
pub mod dynamic_value;
pub mod output_binder;
pub mod params;
pub mod resolved_dynamic_value;
pub mod storage;

pub use dynamic_actions_globals::new_dynamic_actions_callable;

pub fn init_late_bindings() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        calculation::init_dynamic_lambda_calculation();
        storage::init_dynamic_lambda_params_storages();
    });
}
