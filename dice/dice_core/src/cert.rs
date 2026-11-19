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

use crate::arc::Arc;
use crate::env::Premise;
use crate::env::Premises;
use crate::ids::EpsilonToken;
use crate::ids::Key;
use crate::ids::Revision;

/// A certificate: the trace of one completed computation (`incrementality.md` §2.1). It reads
/// "`key`'s compute read exactly `deps` plus the untracked input, observing each at the recorded
/// revision, and produced the value named `revision`."
///
/// A certificate names no version and no branch: it is a fact about the compute, and the core
/// works out where it applies. Several certificates may name one revision, one per computational
/// circumstance that produced an equal value.
#[derive(Allocative, Debug, PartialEq, Eq)]
pub struct Cert<P> {
    pub key: Key,
    pub revision: Revision,
    pub deps: P,
    /// The revision of `key`'s untracked input the value was computed under.
    pub epsilon: EpsilonToken,
}

impl<P: Premises> Cert<P> {
    pub fn new(key: Key, revision: Revision, deps: P, epsilon: EpsilonToken) -> Self {
        Cert {
            key,
            revision,
            deps,
            epsilon,
        }
    }

    /// The certificate's tracked premises. The untracked input, `(key′, epsilon)`, is not among
    /// them.
    pub fn premises(&self) -> impl Iterator<Item = Premise> + '_ {
        self.deps.premises()
    }

    /// Whether two certificates are the same trace. Environments are expected to hand a trace
    /// they already hold back as the same allocation, so the comparison is usually by address.
    pub fn same_trace(this: &Arc<Self>, other: &Arc<Self>) -> bool {
        Arc::ptr_eq(this, other) || **this == **other
    }
}
