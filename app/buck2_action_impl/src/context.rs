/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::context::ANALYSIS_ACTIONS_METHODS_ACTIONS;

use crate::context::copy::analysis_actions_methods_copy;
use crate::context::download::analysis_actions_methods_download;
use crate::context::dynamic_output::analysis_actions_methods_dynamic_output;
use crate::context::run::analysis_actions_methods_run;
use crate::context::unsorted::analysis_actions_methods_unsorted;
use crate::context::write::analysis_actions_methods_write;

pub(crate) mod assembled_dir;
mod copy;
mod download;
pub(crate) mod dynamic_output;
mod execution_group;
pub(crate) mod run;
mod unsorted;
mod write;

/// Functions to allow users to interact with the Actions registry. Accessed via
/// `ctx.actions.<function>`.
///
/// Actions take inputs and produce outputs, mostly using the `artifact` type. Most output filenames
/// can either be artifacts created with `declare_output` or strings that are implicitly converted
/// to output artifacts.
pub(crate) fn init_analysis_action_methods_actions() {
    ANALYSIS_ACTIONS_METHODS_ACTIONS.init(|methods| {
        analysis_actions_methods_copy(methods);
        analysis_actions_methods_download(methods);
        analysis_actions_methods_dynamic_output(methods);
        analysis_actions_methods_run(methods);
        analysis_actions_methods_unsorted(methods);
        analysis_actions_methods_write(methods);
    });
}
