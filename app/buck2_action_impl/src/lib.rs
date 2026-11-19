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
#![feature(try_blocks)]

use std::sync::Once;

mod actions;
mod context;
mod starlark_defs;

pub fn init_late_bindings() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        actions::impls::run::audit_dep_files::init_audit_dep_files();
        actions::impls::run::dep_files::init_dep_file_cache();
        context::init_analysis_action_methods_actions();
        buck2_dynamic_actions::init_late_bindings();
        starlark_defs::init_register_buck2_action_impl_globals();
    });
}
