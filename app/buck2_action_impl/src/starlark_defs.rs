/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_dynamic_actions::attrs_starlark::register_dynamic_attrs;
use buck2_dynamic_actions::dynamic_actions_globals::register_dynamic_actions;
use buck2_dynamic_actions::dynamic_value::register_dynamic_value;
use buck2_dynamic_actions::resolved_dynamic_value::register_resolved_dynamic_value;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_ACTION_IMPL_GLOBALS;

use crate::actions::impls::dep_file_fingerprint::register_dep_file_fingerprint;
use crate::actions::impls::write_json::register_write_json_cli_args;
use crate::context::assembled_dir::register_assembled_dir;

pub(crate) fn init_register_buck2_action_impl_globals() {
    REGISTER_BUCK2_ACTION_IMPL_GLOBALS.init(|globals| {
        register_dynamic_actions(globals);
        register_dynamic_value(globals);
        register_dynamic_attrs(globals);
        register_resolved_dynamic_value(globals);
        register_write_json_cli_args(globals);
        register_dep_file_fingerprint(globals);
        register_assembled_dir(globals);
    });
}
