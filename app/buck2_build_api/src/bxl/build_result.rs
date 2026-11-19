/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;

use allocative::Allocative;
use buck2_core::provider::label::ConfiguredProvidersLabel;

use crate::build::ConfiguredBuildTargetResult;
#[derive(Clone, Debug, Allocative)]
pub enum BxlBuildResult {
    None,
    Built {
        label: ConfiguredProvidersLabel,
        result: ConfiguredBuildTargetResult,
    },
}

impl BxlBuildResult {
    pub fn new(
        label: ConfiguredProvidersLabel,
        result: Option<ConfiguredBuildTargetResult>,
    ) -> Self {
        match result {
            Some(result) => Self::Built { label, result },
            None => Self::None,
        }
    }

    pub fn built(&self) -> Option<&ConfiguredBuildTargetResult> {
        match self {
            Self::None => None,
            Self::Built { result, .. } => Some(result),
        }
    }
}

impl Display for BxlBuildResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BxlBuildResult::None => write!(f, "BxlBuildResult::None"),
            BxlBuildResult::Built { result, .. } => write!(
                f,
                "BxlBuildResult::Built({} outputs, {} errors)",
                result.outputs.len(),
                result.errors.len()
            ),
        }
    }
}
