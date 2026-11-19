/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::invocation_paths::InvocationPaths;

#[derive(Clone)]
pub enum InvocationPathsResult {
    OtherError(buck2_error::Error),
    Paths(InvocationPaths),
    OutsideOfRepo(buck2_error::Error), // this error ignored for creating invocation record for log commands
}

impl InvocationPathsResult {
    pub fn get_result(self) -> buck2_error::Result<InvocationPaths> {
        match self {
            InvocationPathsResult::OtherError(e) => Err(e),
            InvocationPathsResult::Paths(paths) => Ok(paths),
            InvocationPathsResult::OutsideOfRepo(e) => Err(e),
        }
    }
}
