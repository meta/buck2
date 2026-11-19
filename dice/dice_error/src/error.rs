/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::error::Error as StdError;
use std::sync::Arc;

use allocative::Allocative;
use dupe::Dupe;
use thiserror::Error;

use crate::cycles::CycleError;
use crate::cycles::RequestedKey;

#[derive(Clone, Dupe, Debug, Error, Allocative)]
#[error(transparent)]
pub struct DiceError(pub Arc<DiceErrorImpl>);

impl DiceError {
    pub fn invalid_change(key: Arc<dyn RequestedKey>) -> Self {
        DiceError(Arc::new(DiceErrorImpl::ChangedToInvalid(key)))
    }

    pub fn duplicate(key: Arc<dyn RequestedKey>) -> Self {
        DiceError(Arc::new(DiceErrorImpl::DuplicateChange(key)))
    }

    pub fn duplicate_activation_data() -> Self {
        DiceError(Arc::new(DiceErrorImpl::DuplicateActivationData))
    }

    pub fn injected_key_invalidated(key: Arc<dyn RequestedKey>) -> Self {
        DiceError(Arc::new(DiceErrorImpl::InjectedKeyGotInvalidation(key)))
    }

    pub fn page_in_failed(key: String, source: Box<dyn StdError + Send + Sync>) -> Self {
        DiceError(Arc::new(DiceErrorImpl::PageInFailed { key, source }))
    }
}

impl From<CycleError> for DiceError {
    fn from(cycle: CycleError) -> Self {
        DiceError(Arc::new(DiceErrorImpl::Cycle(cycle)))
    }
}

#[derive(Debug, Error, Allocative)]
pub enum DiceErrorImpl {
    #[error("Key `{0}` was marked as changed multiple times on the same transaction.")]
    DuplicateChange(Arc<dyn RequestedKey>),
    #[error("Key `{0}` was reported as changed to an invalid value")]
    ChangedToInvalid(Arc<dyn RequestedKey>),
    #[error("Key `{0}` is an InjectedKey and received an invalidation")]
    InjectedKeyGotInvalidation(Arc<dyn RequestedKey>),
    #[error("Failed to page in the value for key `{key}`")]
    PageInFailed {
        key: String,
        #[source]
        #[allocative(skip)]
        source: Box<dyn StdError + Send + Sync>,
    },
    #[error("Activation data was already provided for this key")]
    DuplicateActivationData,
    /// The requested key is in a dependency cycle with the requesting one, so neither can be
    /// computed.
    #[error("{0}")]
    Cycle(CycleError),
}

pub type DiceResult<T> = Result<T, DiceError>;
