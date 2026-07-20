/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![feature(trait_alias)]
#![feature(try_blocks)]
#![feature(never_type)]
#![feature(mapped_lock_guards)]

use std::sync::Once;

pub(crate) mod bxl;
pub(crate) mod command;
mod commands;
pub(crate) mod profile_command;

pub fn init_late_bindings() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        bxl::starlark_defs::globals::init_bxl_specific_globals();
        bxl::starlark_defs::context::dynamic::init_eval_bxl_for_dynamic_output();
        bxl::calculation::init_bxl_calculation_impl();
        commands::init_bxl_server_commands();
        bxl::starlark_defs::context::anon_target::init_eval_bxl_for_anon_target();
        bxl::starlark_defs::eval_extra::init_bxl_read_config();
    });
}

#[test]
fn init_late_bindings_for_test() {
    #[ctor::ctor(unsafe)]
    fn init() {
        buck2_action_impl::init_late_bindings();
        buck2_anon_target::init_late_bindings();
        buck2_build_api::init_late_bindings();
        buck2_transition::init_late_bindings();
        buck2_interpreter_for_build::init_late_bindings();
        init_late_bindings();
    }
}
