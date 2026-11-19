/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_artifact::artifact::output_type::OutputType;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;

use crate::actions::execute::action_executor::ActionOutputs;

/// This type intentionally does not implement `std::error::Error`. That's because it represents an
/// "incomplete" error - it needs more information like the command results, action keys, etc.
/// before it can be turned into a `buck2_build_api::actions::errors::action_error::ActionError`.
#[derive(Debug)]
pub enum ExecuteError {
    MissingOutputs {
        declared: Vec<ProjectRelativePathBuf>,
    },
    MismatchedOutputs {
        declared: Vec<ProjectRelativePathBuf>,
        real: Vec<ProjectRelativePathBuf>,
    },
    WrongOutputType {
        path: ProjectRelativePathBuf,
        declared: OutputType,
        real: OutputType,
    },
    Error {
        error: buck2_error::Error,
    },
    CommandExecutionError {
        action_outputs: ActionOutputs,
        error: Option<buck2_error::Error>,
    },
}

impl From<buck2_error::Error> for ExecuteError {
    fn from(error: buck2_error::Error) -> Self {
        Self::Error { error }
    }
}
