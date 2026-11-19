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

use std::sync::Once;

pub(crate) mod analysis;
pub(crate) mod aquery;
pub(crate) mod cquery;
mod description;
pub(crate) mod dice;
pub(crate) mod frontend;
pub(crate) mod uquery;

pub fn init_late_bindings() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        analysis::environment::init_query_functions();
        analysis::eval::init_eval_analysis_query();
        aquery::find_matching_action::init_find_matching_action();
        description::init_query_environment_description_by_type();
        frontend::init_query_frontend();
        frontend::init_universe_from_literals();
        cquery::bxl::init_new_bxl_cquery_functions();
        aquery::bxl::init_new_bxl_aquery_functions();
        uquery::bxl::init_new_bxl_uquery_functions();
    });
}
