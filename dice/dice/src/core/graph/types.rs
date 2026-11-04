/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Type definitions for objects relating to the Versioned Graph

use dupe::Dupe;
use variant_name::VariantName;

use crate::api::invalidation_tracking::InvalidationSources;
use crate::arc::Arc;
use crate::core::graph::DiceCert;
use crate::core::graph::revision::EpsilonToken;
use crate::key::DiceKey;
use crate::value::DiceValidValue;
use crate::value::MaybeResident;
use crate::value::MaybeResidentComputedValue;
use crate::versions::VersionNumber;

/// The Key for a Versioned, incremental computation
#[derive(Copy, Clone, Dupe, Debug)]
pub(crate) struct VersionedGraphKey {
    pub(crate) v: VersionNumber,
    pub(crate) k: DiceKey,
}

impl VersionedGraphKey {
    pub(crate) fn new(v: VersionNumber, k: DiceKey) -> Self {
        VersionedGraphKey { v, k }
    }
}

/// A certificate offered by a lookup that could not resolve the key, with its value.
#[derive(Clone, Dupe, Debug)]
pub(crate) struct Candidate {
    pub(crate) cert: Arc<DiceCert>,
    /// The value named by `cert.revision`. Still paged out if nothing has read it back.
    pub(crate) entry: MaybeResident<DiceValidValue>,
    /// Whether the caller may re-establish the certificate (see `docs/incrementality.md` §2.2,
    /// "Intended use") and re-issue it as is. A certificate stamped with another revision of the
    /// key's untracked input cannot be, and is offered only so that a recompute can be compared
    /// against its value.
    pub(crate) revalidatable: bool,
}

/// The core state's answer to a lookup of a key at a version.
#[derive(Debug, VariantName)]
pub(crate) enum VersionedGraphResult {
    /// The key resolves to `value` at the version.
    Match { value: MaybeResidentComputedValue },
    /// The key does not resolve at the version. `candidate` is the nearest certificate the
    /// state retains for the key, if any.
    Unknown {
        candidate: Option<Candidate>,
        /// The key's untracked-input revision, stamped on a newly computed certificate.
        epsilon: EpsilonToken,
        /// The sources of the key's own dirty in force, which anything computed here depends on.
        sources: InvalidationSources,
    },
}

#[cfg(test)]
impl VersionedGraphResult {
    pub(crate) fn unpack_match(&self) -> Option<&MaybeResidentComputedValue> {
        match self {
            Self::Match { value } => Some(value),
            Self::Unknown { .. } => None,
        }
    }
}
