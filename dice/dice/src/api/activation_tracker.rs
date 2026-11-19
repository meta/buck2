/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::time::Duration;
use std::time::Instant;

use dupe::Dupe;

use crate::DynKey;

/// An ActivationTracker can be used to identify which keys were either reused or computed during a
/// transaction.
pub trait ActivationTracker: Send + Sync + 'static {
    /// Receives when a key was activated (computed, or reused). The caller will want to downcast
    /// the key and deps to types they care about. The caller also receives whatever the key passed
    /// to `store_evaluation_data` (if any).
    fn key_activated(
        &self,
        key: &DynKey,
        deps: &mut dyn Iterator<Item = &DynKey>,
        activation_data: ActivationData,
    );

    /// Receives when a paged-out key was paged back in. `start`/`duration` are the wall-clock
    /// span of the hydration (backend fetch + deserialize). `phase` distinguishes value demand
    /// from reads within the key's evaluation (see `PageInPhase`).
    ///
    /// Defaults to a no-op.
    fn key_paged_in(
        &self,
        _key: &DynKey,
        _start: Instant,
        _duration: Duration,
        _phase: PageInPhase,
    ) {
    }
}

/// Where hydration occurs relative to dependency validation and key evaluation.
#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq)]
pub enum PageInPhase {
    /// A caller needs the payload of a key whose revision is already known. This read is
    /// independent of the key's activation: there may be no activation, or it may precede the read.
    ValueDemand,
    /// Recalculation preserved the dependency structure, so the old value is loaded for equality
    /// comparison with the newly computed value.
    AfterRecompute,
}

/// Describes the kind of activation, and possibly carries data passed by the key's evaluation.
pub enum ActivationData {
    /// This key was evaluated. Evaluation data will be passed if the key's evaluation set any.
    Evaluated(Option<Box<dyn Any + Send + Sync + 'static>>),

    /// This key was reused. No data is passed.
    Reused,
}
