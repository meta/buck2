/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The main worker thread for the dice task

use std::sync::Arc;

use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::CriticalSectionGuard;
use dice_futures::cancellation::DisableCancellationGuard;
use dupe::Dupe;
use itertools::Either;

use crate::ActivationData;
use crate::ActivationTracker;
use crate::DynKey;
use crate::epoch::evaluator::KeyEvaluationResult;
use crate::epoch::task::PreviouslyCancelledTask;
use crate::epoch::worker::WorkerCancelled;
use crate::epoch::worker::WorkerResult;
use crate::key::DiceKey;
use crate::key::DiceKeyErased;
use crate::key_index::DiceKeyIndex;
use crate::value::MaybeResidentComputedValue;

/// Represents when we are in a spawned dice task worker and are currently waiting for the previous
/// cancelled instance of this task to finish cancelling.
pub(crate) struct DiceWorkerStateAwaitingPrevious<'a> {
    prevent_cancellation: CriticalSectionGuard<'a>,
}

impl<'a> DiceWorkerStateAwaitingPrevious<'a> {
    pub(crate) fn new(prevent_cancellation: CriticalSectionGuard<'a>) -> Self {
        Self {
            prevent_cancellation,
        }
    }

    pub(crate) fn previously_finished(
        self,
        value: MaybeResidentComputedValue,
    ) -> WorkerResult<DiceWorkerStateFinishedAndCached> {
        let guard = self.prevent_cancellation.try_disable_cancellation();
        finish_with_cached_value(value, guard)
    }

    pub(crate) async fn previously_cancelled(self) -> DiceWorkerStateLookupNode {
        self.prevent_cancellation.exit_critical_section().await;

        DiceWorkerStateLookupNode {}
    }

    pub(crate) async fn no_previous_task(self) -> DiceWorkerStateLookupNode {
        self.prevent_cancellation.exit_critical_section().await;

        DiceWorkerStateLookupNode {}
    }

    pub(crate) async fn await_previous(
        self,
        previous: PreviouslyCancelledTask,
    ) -> Either<WorkerResult<DiceWorkerStateFinishedAndCached>, DiceWorkerStateLookupNode> {
        // A cancelled task can still race to completion before its cancellation lands. If the
        // previous generation finished with a value, reuse it instead of recomputing: every
        // generation of this task computes the same key at the same version, so the value is valid
        // regardless of which generation produced it. Crucially, reusing it also avoids re-running
        // the computation and so a *second* write of the same value into core state — duplicate
        // writes aren't necessarily fatal, but they're hard to reason about, so we avoid them.
        if let Some(v) = previous.await_termination().await {
            return Either::Left(self.previously_finished(v));
        }

        // Otherwise the previous generation actually cancelled; fall through and recompute.
        Either::Right(self.previously_cancelled().await)
    }
}

fn finish_with_cached_value(
    value: MaybeResidentComputedValue,
    disable_cancellation: Option<DisableCancellationGuard>,
) -> WorkerResult<DiceWorkerStateFinishedAndCached> {
    match disable_cancellation {
        None => Err(WorkerCancelled),
        Some(g) => Ok(DiceWorkerStateFinishedAndCached {
            value,
            _prevent_cancellation: g,
        }),
    }
}

/// Represents when we are currently looking up the current requested key from the core state, and
/// are waiting for it to respond.
pub(crate) struct DiceWorkerStateLookupNode {}

impl DiceWorkerStateLookupNode {
    pub(crate) fn checking_deps(self) -> DiceWorkerStateCheckingDeps {
        DiceWorkerStateCheckingDeps {}
    }

    pub(crate) fn lookup_dirtied(self) -> DiceWorkerStateEvaluating {
        DiceWorkerStateEvaluating {}
    }

    pub(crate) fn lookup_matches(
        self,
        cancellations: &CancellationContext,
        value: MaybeResidentComputedValue,
    ) -> WorkerResult<DiceWorkerStateFinishedAndCached> {
        let guard = cancellations.try_disable_cancellation();
        finish_with_cached_value(value, guard)
    }
}

/// When the spawned dice task worker is checking if the dependencies have changed since the last
/// time this node was verified, and are waiting for the results of the dependency re-computation.
pub(crate) struct DiceWorkerStateCheckingDeps {}

impl DiceWorkerStateCheckingDeps {
    pub(crate) fn deps_not_match(self) -> DiceWorkerStateEvaluating {
        DiceWorkerStateEvaluating {}
    }

    pub(crate) fn deps_match(
        self,
        cancellations: &CancellationContext,
    ) -> WorkerResult<DiceWorkerStateFinished> {
        let guard = match cancellations.try_disable_cancellation() {
            Some(g) => g,
            None => return Err(WorkerCancelled),
        };

        Ok(DiceWorkerStateFinished {
            _prevent_cancellation: guard,
        })
    }
}

/// When the spawned dice worker is currently actively evaluating the `Key::compute` function
pub(crate) struct DiceWorkerStateEvaluating {}

impl DiceWorkerStateEvaluating {
    pub(crate) fn finished(
        self,
        cancellations: &CancellationContext,
        result: KeyEvaluationResult,
        activation_data: ActivationData,
    ) -> WorkerResult<DiceWorkerStateFinishedEvaluating> {
        let guard = match cancellations.try_disable_cancellation() {
            Some(g) => g,
            None => return Err(WorkerCancelled),
        };

        Ok(DiceWorkerStateFinishedEvaluating {
            state: DiceWorkerStateFinished {
                _prevent_cancellation: guard,
            },
            activation_data,
            result,
        })
    }
}

/// When the spawned dice worker has just finished evaluating the `Key::compute` function
pub(crate) struct DiceWorkerStateFinishedEvaluating {
    pub(crate) state: DiceWorkerStateFinished,
    pub(crate) activation_data: ActivationData,
    pub(crate) result: KeyEvaluationResult,
}

/// When the spawned dice worker is finished checking dependencies or finished computing the key.
/// At this point, the value of the node is known. We are just waiting for core state to finish
/// updating the caches and return the correct instance of the value.
pub(crate) struct DiceWorkerStateFinished {
    _prevent_cancellation: DisableCancellationGuard,
}

impl DiceWorkerStateFinished {
    pub(crate) fn cached(
        self,
        value: MaybeResidentComputedValue,
        activation_info: Option<ActivationInfo>,
    ) -> DiceWorkerStateFinishedAndCached {
        if let Some(activation_info) = activation_info {
            activation_info.activation_tracker.key_activated(
                DynKey::ref_cast(&activation_info.key),
                &mut activation_info.deps.iter().map(DynKey::ref_cast),
                activation_info.activation_data,
            )
        }

        DiceWorkerStateFinishedAndCached {
            value,
            _prevent_cancellation: self._prevent_cancellation,
        }
    }
}

pub(crate) struct ActivationInfo {
    activation_tracker: Arc<dyn ActivationTracker>,
    key: DiceKeyErased,
    deps: Vec<DiceKeyErased>,
    activation_data: ActivationData,
}

impl ActivationInfo {
    pub(crate) fn new<'a>(
        key_index: &DiceKeyIndex,
        activation_tracker: &Option<Arc<dyn ActivationTracker>>,
        key: DiceKey,
        deps: impl Iterator<Item = DiceKey> + 'a,
        activation_data: ActivationData,
    ) -> Option<ActivationInfo> {
        if let Some(activation_tracker) = activation_tracker {
            let key = key_index.get(key).dupe();
            let deps = deps.map(|dep| key_index.get(dep).dupe()).collect();

            Some(ActivationInfo {
                activation_tracker: activation_tracker.dupe(),
                key,
                deps,
                activation_data,
            })
        } else {
            None
        }
    }
}

/// When the spawned dice worker is done computing and saving the value to core state cache.
/// The final value is known.
pub(crate) struct DiceWorkerStateFinishedAndCached {
    pub(crate) value: MaybeResidentComputedValue,
    pub(crate) _prevent_cancellation: DisableCancellationGuard,
}
