/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_BUILD_API_GLOBALS;
use buck2_interpreter::downstream_crate_starlark_defs::REGISTER_BUCK2_BUILD_API_INTERNALS;
use starlark::environment::GlobalsBuilder;

use crate::actions::errors::error_handler::register_action_error_handler_for_testing;
use crate::actions::errors::error_handler::register_action_error_types;
use crate::interpreter::rule_defs::artifact::artifact_type::register_artifact;
use crate::interpreter::rule_defs::artifact::starlark_artifact_value::register_artifact_value;
use crate::interpreter::rule_defs::artifact::starlark_output_artifact::register_output_artifact;
use crate::interpreter::rule_defs::artifact::starlark_promise_artifact::register_promise_artifact;
use crate::interpreter::rule_defs::artifact_tagging::artifact_tag::register_artifact_tag;
use crate::interpreter::rule_defs::artifact_tagging::register_tagged_command_line;
use crate::interpreter::rule_defs::artifact_tagging::register_tagged_value;
use crate::interpreter::rule_defs::cmd_args::register_cmd_args;
use crate::interpreter::rule_defs::cmd_args::register_command_line_inputs;
use crate::interpreter::rule_defs::command_executor_config::register_command_executor_config;
use crate::interpreter::rule_defs::context::register_analysis_context;
use crate::interpreter::rule_defs::digest_config::register_digest_config_type;
use crate::interpreter::rule_defs::provider::callable::register_provider;
use crate::interpreter::rule_defs::provider::collection::register_provider_collection;
use crate::interpreter::rule_defs::provider::dependency::register_dependency;
use crate::interpreter::rule_defs::provider::registration::register_builtin_providers;
use crate::interpreter::rule_defs::register_rule_defs;
use crate::interpreter::rule_defs::required_test_local_resource::register_required_test_local_resource;
use crate::interpreter::rule_defs::resolved_macro::register_string_with_macros;
use crate::interpreter::rule_defs::transitive_set::globals::register_transitive_set_types;
use crate::interpreter::rule_defs::transitive_set::transitive_set_definition::register_transitive_set;
use crate::interpreter::rule_defs::validation_spec::register_validation_spec;

fn register_build_api_globals(globals: &mut GlobalsBuilder) {
    register_builtin_providers(globals);
    register_cmd_args(globals);
    register_command_executor_config(globals);
    register_provider(globals);
    register_provider_collection(globals);
    register_rule_defs(globals);
    register_string_with_macros(globals);
    register_transitive_set(globals);
    register_transitive_set_types(globals);
    register_analysis_context(globals);
    register_dependency(globals);
    register_artifact(globals);
    register_artifact_tag(globals);
    register_artifact_value(globals);
    register_output_artifact(globals);
    register_promise_artifact(globals);
    register_action_error_types(globals);
    register_validation_spec(globals);
    register_required_test_local_resource(globals);
    register_digest_config_type(globals);
    register_tagged_value(globals);
    register_tagged_command_line(globals);
    register_command_line_inputs(globals);
}

fn register_build_api_internals(globals: &mut GlobalsBuilder) {
    register_action_error_handler_for_testing(globals);
}

pub(crate) fn init_register_build_api_globals() {
    REGISTER_BUCK2_BUILD_API_GLOBALS.init(register_build_api_globals);
    REGISTER_BUCK2_BUILD_API_INTERNALS.init(register_build_api_internals);
}
