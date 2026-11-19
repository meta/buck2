/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_hash::BuckFixedSortedMap;
use pagable::Pagable;

use crate::configuration::data::ConfigurationData;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum TransitionAppliedError {
    #[error("Transition object is declared split, but transition to one is needed in this context")]
    SplitWhereSingleExpected,
    #[error(
        "Transition object is declared non-split, but split transition is needed in this context"
    )]
    SingleWhereSplitExpected,
}

/// Result of `transition` function application to a configuration.
#[derive(PartialEq, Eq, Hash, Debug, Allocative, Pagable)]
pub enum TransitionApplied {
    /// Transition to single configuration.
    Single(ConfigurationData),
    /// Split transition.
    Split(BuckFixedSortedMap<String, ConfigurationData>),
}

impl TransitionApplied {
    pub fn single(&self) -> buck2_error::Result<&ConfigurationData> {
        match self {
            TransitionApplied::Single(configuration) => Ok(configuration),
            _ => Err(TransitionAppliedError::SplitWhereSingleExpected.into()),
        }
    }

    pub fn split(&self) -> buck2_error::Result<&BuckFixedSortedMap<String, ConfigurationData>> {
        match self {
            TransitionApplied::Split(configurations) => Ok(configurations),
            _ => Err(TransitionAppliedError::SingleWhereSplitExpected.into()),
        }
    }
}
