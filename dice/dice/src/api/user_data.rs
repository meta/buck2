/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use allocative::Allocative;
use dice_futures::spawner::Spawner;
use dice_futures::spawner::TokioSpawner;

use crate::api::activation_tracker::ActivationTracker;
use crate::api::data::DiceData;
use crate::api::events::DiceEvent;
use crate::api::events::DiceEventListener;

/// Includes all user related computation-specific data.
///
/// Lives in an `Arc` that every spawned computation dupes, so the alignment keeps the `Arc`'s
/// counts off the cache line of the data every computation reads.
#[derive(Allocative)]
#[repr(align(128))]
pub struct UserComputationData {
    /// The DiceData provides a spot for users to attach whatever extra things they want.
    ///
    /// This can contain arbitrary data from users that will not be part of the dice graph.
    /// As an example, users may want to inject some form of event dispatcher to send events from their computations.
    pub data: DiceData,
    pub tracker: Arc<dyn DiceEventListener>,
    #[allocative(skip)]
    pub spawner: Arc<dyn Spawner<Self>>,

    #[allocative(skip)]
    pub activation_tracker: Option<Arc<dyn ActivationTracker>>,

    /// We require that UserComputationData always be constructed with `..Default::default()`
    pub _requires_default: RequireDefault,
}

#[derive(Allocative)]
pub struct RequireDefault(());

impl UserComputationData {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for UserComputationData {
    fn default() -> Self {
        Self {
            data: DiceData::new(),
            tracker: Arc::new(NoOpTracker),
            spawner: Arc::new(TokioSpawner),
            activation_tracker: None,
            _requires_default: RequireDefault(()),
        }
    }
}

#[derive(Allocative)]
pub(crate) struct NoOpTracker;

impl DiceEventListener for NoOpTracker {
    fn event(&self, _ev: DiceEvent) {}
}
