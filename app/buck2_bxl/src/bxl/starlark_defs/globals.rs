/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Starlark globals for BXL.

use buck2_build_api::bxl::select::register_select_types;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_BXL_GLOBALS;
use starlark::environment::GlobalsBuilder;

use crate::bxl::starlark_defs::bxl_function::register_bxl_main_function;
use crate::bxl::starlark_defs::bxl_function::register_bxl_prefixed_main_function;
use crate::bxl::starlark_defs::cli_args;
use crate::bxl::starlark_defs::context::anon_target::register_anon_rule;
use crate::bxl::starlark_defs::context::dynamic::register_dynamic_actions;
use crate::bxl::starlark_defs::functions::register_artifact_function;
use crate::bxl::starlark_defs::functions::register_error_handling_function;
use crate::bxl::starlark_defs::functions::register_instant_function;
use crate::bxl::starlark_defs::functions::register_read_package_value_function;
use crate::bxl::starlark_defs::functions::register_read_package_visibility_functions;
use crate::bxl::starlark_defs::functions::register_target_function;
use crate::bxl::starlark_defs::type_names::register_bxl_type_names_in_bxl_namespace;

fn bxl_namespace(g: &mut GlobalsBuilder) {
    register_bxl_main_function(g);
    g.namespace("cli_args", cli_args::register_cli_args_module);
    // TODO(nga): add `main` function here.
    register_artifact_function(g);
    register_target_function(g);
    register_instant_function(g);
    register_error_handling_function(g);
    register_read_package_value_function(g);
    register_read_package_visibility_functions(g);
    register_bxl_type_names_in_bxl_namespace(g);
    register_select_types(g);
    register_dynamic_actions(g);
    register_anon_rule(g);
}

pub(crate) fn init_bxl_specific_globals() {
    REGISTER_BUCK2_BXL_GLOBALS.init(|g| {
        g.namespace("bxl", bxl_namespace);
        // TODO(nga): move these into `bxl` namespace.
        g.namespace("cli_args", cli_args::register_cli_args_module);
        register_bxl_prefixed_main_function(g);
        register_artifact_function(g);
        register_target_function(g);
        register_instant_function(g);
        register_error_handling_function(g);
    });
}
