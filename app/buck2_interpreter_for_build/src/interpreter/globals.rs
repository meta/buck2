/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_ACTION_IMPL_GLOBALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_ANON_TARGETS_GLOBALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_BUILD_API_GLOBALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_BUILD_API_INTERNALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_BXL_GLOBALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_CFG_CONSTRUCTOR_GLOBALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_TRANSITION_GLOBALS;
use buck2_interpreter::starlark_promise::register_promise;
use buck2_interpreter::types::cell_path::register_cell_path;
use buck2_interpreter::types::cell_root::register_cell_root;
use buck2_interpreter::types::configured_providers_label::register_providers_label;
use buck2_interpreter::types::package_path::register_package_path;
use buck2_interpreter::types::project_root::register_project_root;
use buck2_interpreter::types::regex::register_buck_regex;
use buck2_interpreter::types::target_label::register_target_label;
use buck2_util::late_binding::LateBinding;
use starlark::environment::GlobalsBuilder;
use starlark::environment::LibraryExtension;

use crate::attrs::attrs_global::register_attrs;
use crate::execution_group::register_execution_group;
use crate::interpreter::functions::dedupe::register_dedupe;
use crate::interpreter::functions::host_info::register_host_info;
use crate::interpreter::functions::internals::register_internals;
use crate::interpreter::functions::load_symbols::register_load_symbols;
use crate::interpreter::functions::path::register_path;
use crate::interpreter::functions::read_config::register_read_config;
use crate::interpreter::functions::regex::register_regex;
use crate::interpreter::functions::sha1::register_sha1;
use crate::interpreter::functions::sha256::register_sha256;
use crate::interpreter::functions::soft_error::register_soft_error;
use crate::interpreter::functions::starlark::register_set_starlark_peak_allocated_byte_limit;
use crate::interpreter::functions::warning::register_warning;
use crate::interpreter::functions::xml::register_xml;
use crate::interpreter::natives::register_module_natives;
use crate::interpreter::selector::register_select;
use crate::interpreter::selector::register_select_internal;
use crate::plugins::register_plugins;
use crate::rule::register_rule_function;
use crate::super_package::defs::register_package_natives;
use crate::super_package::package_value::register_read_package_value;

fn from_late_binding(l: &LateBinding<fn(&mut GlobalsBuilder)>, builder: &mut GlobalsBuilder) {
    if let Ok(v) = l.get() {
        v(builder);
    }
}

// NOTE: Semantically, `register_load_natives`, `register_analysis_natives`, `register_bxl_natives`,
// and `starlark_library_extensions_for_buck2` are all the same, since all symbols are available
// everywhere. However, we distinguish between them for the purpose of generating documentation.

pub fn register_load_natives(builder: &mut GlobalsBuilder) {
    from_late_binding(&REGISTER_BUCK2_CFG_CONSTRUCTOR_GLOBALS, builder);
    from_late_binding(&REGISTER_BUCK2_TRANSITION_GLOBALS, builder);
    register_module_natives(builder);
    register_host_info(builder);
    register_read_config(builder);
    register_read_package_value(builder);
    register_soft_error(builder);
    register_package_natives(builder);
    register_warning(builder);
    register_regex(builder);
    register_buck_regex(builder);
    register_load_symbols(builder);
    register_rule_function(builder);
    register_attrs(builder);
    register_plugins(builder);
    register_execution_group(builder);
    register_providers_label(builder);
    register_cell_path(builder);
    register_cell_root(builder);
    register_package_path(builder);
    register_project_root(builder);
    register_target_label(builder);
    register_path(builder);
    register_select(builder);
    register_sha1(builder);
    register_sha256(builder);
    register_dedupe(builder);
    register_set_starlark_peak_allocated_byte_limit(builder);
}

pub fn register_analysis_natives(builder: &mut GlobalsBuilder) {
    from_late_binding(&REGISTER_BUCK2_ACTION_IMPL_GLOBALS, builder);
    from_late_binding(&REGISTER_BUCK2_BUILD_API_GLOBALS, builder);
    register_promise(builder);
    from_late_binding(&REGISTER_BUCK2_ANON_TARGETS_GLOBALS, builder);
}

pub fn register_bxl_natives(builder: &mut GlobalsBuilder) {
    from_late_binding(&REGISTER_BUCK2_BXL_GLOBALS, builder);
}

pub fn starlark_library_extensions_for_buck2() -> &'static [LibraryExtension] {
    &[
        LibraryExtension::Breakpoint,
        LibraryExtension::Debug,
        LibraryExtension::EnumType,
        LibraryExtension::Filter,
        LibraryExtension::Json,
        LibraryExtension::Map,
        LibraryExtension::Partial,
        LibraryExtension::Pprint,
        LibraryExtension::Pstr,
        LibraryExtension::Prepr,
        LibraryExtension::Print,
        LibraryExtension::RecordType,
        LibraryExtension::StructType,
        LibraryExtension::Typing,
        LibraryExtension::Internal,
        LibraryExtension::CallStack,
        LibraryExtension::SetType,
        LibraryExtension::NamespaceType,
    ]
}

fn register_all_natives(builder: &mut GlobalsBuilder) {
    register_load_natives(builder);
    register_analysis_natives(builder);
    register_bxl_natives(builder);
    for ext in starlark_library_extensions_for_buck2() {
        ext.add(builder);
    }
}

fn register_all_internals(builder: &mut GlobalsBuilder) {
    register_internals(builder);
    from_late_binding(&REGISTER_BUCK2_BUILD_API_INTERNALS, builder);
    register_select_internal(builder);
    register_xml(builder);
}

/// The standard set of globals that is available in all files.
///
/// This does not include the implicit prelude and cell imports which are only available in `BUCK`
/// files, but does include everything else.
///
/// Note: As long as starlark/buck have any notion of reference equality, it is important for
/// correctness that this be called just once. The result should be accessed via
/// `ctx.get_global_interpreter_state()`
pub(crate) fn base_globals() -> GlobalsBuilder {
    let mut global_env = GlobalsBuilder::standard().with(register_all_natives);
    global_env.namespace("__internal__", |x| {
        register_all_internals(x);
    });
    global_env.namespace("__buck2_builtins__", |x| {
        register_all_natives(x);
    });
    global_env
}
