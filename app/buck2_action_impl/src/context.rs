/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_build_api::interpreter::rule_defs::context::ANALYSIS_ACTIONS_MEMBERS;

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
    // SAFETY: Late bindings are set as the program starts, before anything is evaluated.
    unsafe {
        ANALYSIS_ACTIONS_MEMBERS.extend(&copy::ANALYSIS_ACTIONS_METHODS_COPY_MEMBERS);
        ANALYSIS_ACTIONS_MEMBERS.extend(&download::ANALYSIS_ACTIONS_METHODS_DOWNLOAD_MEMBERS);
        ANALYSIS_ACTIONS_MEMBERS
            .extend(&dynamic_output::ANALYSIS_ACTIONS_METHODS_DYNAMIC_OUTPUT_MEMBERS);
        ANALYSIS_ACTIONS_MEMBERS.extend(&run::ANALYSIS_ACTIONS_METHODS_RUN_MEMBERS);
        ANALYSIS_ACTIONS_MEMBERS.extend(&unsorted::ANALYSIS_ACTIONS_METHODS_UNSORTED_MEMBERS);
        ANALYSIS_ACTIONS_MEMBERS.extend(&write::ANALYSIS_ACTIONS_METHODS_WRITE_MEMBERS);
    }
}
