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

use std::time::Instant;

use dice_error::cycles::CycleError;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::Future;
use futures::FutureExt;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::pin_mut;
use futures::stream;
use futures::stream::FuturesUnordered;
use itertools::Either;
use variant_name::VariantName;

use crate::DynKey;
use crate::api::activation_tracker::ActivationData;
use crate::api::activation_tracker::PageInPhase;
use crate::api::invalidation_tracking::InvalidationSources;
use crate::core::graph::revision::EpsilonToken;
use crate::core::graph::types::VersionedGraphKey;
use crate::core::graph::types::VersionedGraphResult;
use crate::core::state::CoreStateHandle;
use crate::deps::graph::DepEdge;
use crate::deps::graph::SeriesParallelDeps;
use crate::deps::iterator::SeriesParallelDepsIteratorItem;
use crate::epoch::cache::ProjectionValidation;
use crate::epoch::cycles::ensure_cycle_detector_running;
use crate::epoch::evaluator::TransactionData;
use crate::epoch::evaluator::ValueDemand;
use crate::epoch::task::PreviouslyCancelledTask;
use crate::epoch::task::dice::PreparedDiceTask;
use crate::epoch::task::dice::spawn_prepared_task;
use crate::epoch::task::promise::DicePromise;
use crate::epoch::worker::state::ActivationInfo;
use crate::epoch::worker::state::DiceWorkerStateAwaitingPrevious;
use crate::epoch::worker::state::DiceWorkerStateEvaluating;
use crate::epoch::worker::state::DiceWorkerStateFinishedAndCached;
use crate::epoch::worker::state::DiceWorkerStateFinishedEvaluating;
use crate::epoch::worker::state::DiceWorkerStateLookupNode;
use crate::key::DiceKey;
use crate::key::DiceKeyErased;
use crate::key::ParentKey;
use crate::value::MaybeResidentComputedValue;
use crate::value::ResidentComputedValue;

pub(crate) mod state;

#[cfg(test)]
mod tests;

/// An error indicating that the worker this is running in the context of was cancelled.
pub(crate) struct WorkerCancelled;

pub(crate) type WorkerResult<T> = Result<T, WorkerCancelled>;

/// The worker on the spawned dice task
///
/// Manages all the handling of the results of a specific key, performing the recomputation
/// if necessary
///
/// The computation of an identical request (same key and version) is
/// automatically deduplicated, so that identical requests share the same set of
/// work. It is guaranteed that there is at most one computation in flight at a
/// time if they share the same key and version.
pub(crate) struct DiceTaskWorker {
    k: DiceKey,
    eval: TransactionData,
    mode: DiceTaskMode,
}

enum DiceTaskMode {
    Lookup,
    /// A failed demand already ruled out reuse. Run the normal compute/write path directly,
    /// with the ε and the sources a lookup would have handed out.
    Recompute(EpsilonToken, InvalidationSources),
}

impl DiceTaskWorker {
    pub(crate) fn new(k: DiceKey, eval: TransactionData) -> Self {
        Self {
            k,
            eval,
            mode: DiceTaskMode::Lookup,
        }
    }

    pub(crate) fn new_recompute(
        k: DiceKey,
        eval: TransactionData,
        epsilon: EpsilonToken,
        sources: InvalidationSources,
    ) -> Self {
        Self {
            k,
            eval,
            mode: DiceTaskMode::Recompute(epsilon, sources),
        }
    }

    pub(crate) fn spawn<'d>(
        self,
        prepared_task: PreparedDiceTask<'d>,
        previously_cancelled_task: Option<PreviouslyCancelledTask>,
    ) -> DicePromise<'d> {
        let spawner = self.eval.user_data.spawner.dupe();
        let spawner_ctx = self.eval.user_data.dupe();
        let state_handle = self.eval.dice.state_handle.dupe();

        ensure_cycle_detector_running(&self.eval);

        spawn_prepared_task(prepared_task, &*spawner, &spawner_ctx, move |handle| {
            // NOTE: important to run prevent cancellation eagerly in the sync scope to prevent
            // cancellations so that we don't cancel the current task before we finish waiting
            // for the previously cancelled task
            let cancellations = handle.cancellation_ctx();
            let prevent_cancellation = cancellations.enter_critical_section();
            let state = DiceWorkerStateAwaitingPrevious::new(prevent_cancellation);

            async move {
                let previous_result = match previously_cancelled_task {
                    Some(v) => state.await_previous(v).await,
                    None => Either::Right(state.no_previous_task().await),
                };

                let result = match previous_result {
                    Either::Left(previous_result) => {
                        // previous result actually finished
                        previous_result
                    }
                    Either::Right(state) => self.do_work(cancellations, state_handle, state).await,
                };

                if result.is_ok() {
                    self.eval.version_state.cache().note_task_completed();
                }
                handle.finished(result.map(|state| state.value));
            }
            .boxed()
        })
    }

    /// This is the primary flow of how a key is computed or re-computed.
    pub(crate) async fn do_work(
        &self,
        cancellations: &CancellationContext,
        state_handle: CoreStateHandle,
        task_state: DiceWorkerStateLookupNode,
    ) -> WorkerResult<DiceWorkerStateFinishedAndCached> {
        let v = self.eval.version_state.get_version();

        // handle cancelled/cache hits before sending started events
        let (candidate, epsilon, own_sources) = match self.mode {
            DiceTaskMode::Lookup => match state_handle
                .lookup_key(VersionedGraphKey::new(v, self.k))
                .await
            {
                VersionedGraphResult::Match { value } => {
                    return task_state.lookup_matches(cancellations, value);
                }
                VersionedGraphResult::Unknown {
                    candidate,
                    epsilon,
                    sources,
                } => (candidate, epsilon, sources),
            },
            DiceTaskMode::Recompute(epsilon, sources) => (None, epsilon, sources),
        };

        self.eval.started(self.k);
        scopeguard::defer! {
            self.eval.finished(self.k);
        };

        let revalidatable = candidate.as_ref().filter(|c| c.revalidatable);

        let (task_state, deps_check_continuables) = match revalidatable {
            Some(to_revalidate) => {
                let task_state = task_state.checking_deps();

                self.eval.check_deps_started(self.k);

                let check_deps_result = {
                    scopeguard::defer! {
                        self.eval.check_deps_finished(self.k);
                    }
                    check_dependencies(
                        &self.eval,
                        ParentKey::Some(self.k),
                        &to_revalidate.cert.deps,
                    )
                    .await
                };

                match check_deps_result {
                    CheckDependenciesResult::NoChange { .. } => {
                        let mut invalidation_sources =
                            check_deps_result.unwrap_no_change_invalidation_sources();
                        invalidation_sources.update(own_sources);

                        let task_state = task_state.deps_match(cancellations)?;
                        let activation_info = self.activation_info(
                            to_revalidate.cert.deps.iter_keys(),
                            ActivationData::Reused,
                        );
                        let response = state_handle
                            .revalidate(
                                VersionedGraphKey::new(v, self.k),
                                self.eval.storage_type(self.k),
                                to_revalidate.dupe(),
                                invalidation_sources,
                            )
                            .await;

                        return Ok(task_state.cached(response, activation_info));
                    }
                    CheckDependenciesResult::NoDeps => {
                        // TODO(cjhopman): Why do we treat nodeps as deps not matching? There seems to be some
                        // implicit meaning to a node having no deps at this point, but it's unclear what that is.
                        (task_state.deps_not_match(), None)
                    }
                    CheckDependenciesResult::Changed { continuables } => {
                        (task_state.deps_not_match(), Some(continuables))
                    }
                }
            }
            None => (task_state.lookup_dirtied(), None),
        };

        let DiceWorkerStateFinishedEvaluating {
            state,
            activation_data,
            result,
        } = self.compute(cancellations, task_state, own_sources).await?;

        // explicitly drop this here to make it clear that its important that we hold onto it, it
        // otherwise appears unused, but we don't want to cancel anything that it has started requesting
        // before compute finishes.
        // TODO(cjhopman): we could be polling this future, it might eagerly request deps more quickly than
        // the compute would.
        drop(deps_check_continuables);

        let activation_info = self.activation_info(result.deps.iter_keys(), activation_data);

        let res = {
            match result.value.into_valid_value() {
                Ok(value) => {
                    let v = self.eval.version_state.get_version();
                    // If the dependencies still match and equality can reuse the old value,
                    // restore it so `update_computed` can compare it with the recomputed value.
                    if let Some(stale) = candidate.as_ref()
                        && let Some(data_key) = stale.entry.data_key()
                        && result.deps.equal_ignoring_revisions(&stale.cert.deps)
                        && !self
                            .eval
                            .dice
                            .key_index
                            .get(self.k)
                            .equality_is_always_unequal()
                    {
                        if let Err(e) = self
                            .hydrate_and_rehydrate(
                                &state_handle,
                                data_key,
                                PageInPhase::AfterRecompute,
                            )
                            .await
                        {
                            self.eval.hydration_failed(self.k, &e);
                        }
                    }
                    state_handle
                        .update_computed(
                            VersionedGraphKey::new(v, self.k),
                            result.storage,
                            value,
                            result.deps.certify(),
                            epsilon,
                            result.invalidation_sources,
                        )
                        .await
                }
                Err(value) => {
                    ResidentComputedValue::new_for_transient(value, result.invalidation_sources)
                }
            }
        };

        Ok(state.cached(res.into_computed(), activation_info))
    }

    async fn compute(
        &self,
        cancellations: &CancellationContext,
        task_state: DiceWorkerStateEvaluating,
        own_sources: InvalidationSources,
    ) -> WorkerResult<DiceWorkerStateFinishedEvaluating> {
        self.eval.compute_started(self.k);
        scopeguard::defer! {
            self.eval.compute_finished(self.k);
        };

        self.eval
            .evaluate(cancellations, self.k, task_state, own_sources)
            .await
    }

    fn activation_info<'a>(
        &self,
        deps: impl Iterator<Item = DiceKey> + 'a,
        data: ActivationData,
    ) -> Option<ActivationInfo> {
        ActivationInfo::new(
            &self.eval.dice.key_index,
            &self.eval.user_data.activation_tracker,
            self.k,
            deps,
            data,
        )
    }

    /// Restore a paged-out candidate for equality comparison with the newly computed value.
    /// A failed read is reported by the caller, which continues with the already computed
    /// value. Paged-out candidates require configured storage.
    async fn hydrate_and_rehydrate(
        &self,
        state_handle: &CoreStateHandle,
        data_key: pagable::DataKey,
        phase: PageInPhase,
    ) -> anyhow::Result<crate::value::DiceValidValue> {
        let storage = self
            .eval
            .dice
            .pagable_storage
            .as_ref()
            .expect("paged-out lookup result requires DiceStorage to be configured");
        let key_dyn = self.eval.dice.key_index.get(self.k);
        let hydrate_start = Instant::now();
        let hydrated = storage.hydrate(key_dyn, data_key).await?;
        let value = hydrated.value.dupe();
        // Forward the phase so consumers can place hydration relative to dependency and compute
        // work.
        if let Some(activation_tracker) = self.eval.user_data.activation_tracker.as_ref() {
            activation_tracker.key_paged_in(
                DynKey::ref_cast(key_dyn),
                hydrate_start,
                hydrate_start.elapsed(),
                phase,
            );
        }
        state_handle.rehydrate(self.k, data_key, hydrated);
        Ok(value)
    }
}

/// Used for checking if dependencies have changed since the previously checked version.
async fn check_dependencies<'a>(
    eval: &'a TransactionData,
    parent_key: ParentKey,
    deps: &'a SeriesParallelDeps,
) -> CheckDependenciesResult<'a> {
    async fn drain_continuables<'a, Fut: Future<Output = CheckDependenciesResult<'a>>>(
        inner: BoxFuture<'a, ()>,
        parallel: FuturesUnordered<Fut>,
    ) {
        let parallel = parallel.map(|_| ());
        let combined = stream::select(inner.into_stream(), parallel);
        pin_mut!(combined);
        while combined.next().await.is_some() {}
    }

    fn check_dependencies_series<'a>(
        eval: &'a TransactionData,
        parent_key: ParentKey,
        deps: impl Iterator<Item = SeriesParallelDepsIteratorItem<'a>> + Send + 'a,
    ) -> BoxFuture<'a, CheckDependenciesResult<'a>> {
        let mut invalidation_sources = InvalidationSources::NONE;
        async move {
            for v in deps {
                match v {
                    SeriesParallelDepsIteratorItem::Key(edge) => {
                        match check_dependency(eval, parent_key, edge).await {
                            CheckDependencyResult::NoChange(dep_sources) => {
                                invalidation_sources.update(dep_sources);
                            }
                            CheckDependencyResult::Changed => {
                                return CheckDependenciesResult::Changed {
                                    continuables: std::future::ready(()).boxed(),
                                };
                            }
                        }
                    }
                    SeriesParallelDepsIteratorItem::Parallel(p) => {
                        let mut futures: FuturesUnordered<_> = p
                            .map(|deps| check_dependencies_series(eval, parent_key, deps).boxed())
                            .collect();

                        while let Some(v) = futures.next().await {
                            match v {
                                CheckDependenciesResult::NoChange {
                                    invalidation_sources: deps_sources,
                                } => {
                                    invalidation_sources.update(deps_sources);
                                }
                                CheckDependenciesResult::NoDeps => {}
                                CheckDependenciesResult::Changed { continuables } => {
                                    return CheckDependenciesResult::Changed {
                                        continuables: drain_continuables(continuables, futures)
                                            .boxed(),
                                    };
                                }
                            }
                        }
                    }
                }
            }
            CheckDependenciesResult::NoChange {
                invalidation_sources,
            }
        }
        .boxed()
    }

    if deps.is_empty() {
        return CheckDependenciesResult::NoDeps;
    }

    check_dependencies_series(eval, parent_key, deps.iter()).await
}

enum CheckDependencyResult {
    Changed,
    NoChange(InvalidationSources),
}

async fn check_dependency(
    eval: &TransactionData,
    parent_key: ParentKey,
    edge: DepEdge,
) -> CheckDependencyResult {
    // A projection task recomputes the projection from its base's value and cannot fail, so a
    // base that would have to be read is read here, where a failed read has an answer.
    if let DiceKeyErased::Projection(proj) = eval.dice.key_index.get(edge.key) {
        // The checked key waits for the base here on the projection's behalf, so the wait is
        // registered as its own: it is the one stuck if the base in turn waits on it. A cycle
        // fails this wait like the one on the dep itself below.
        let base = match eval.bring_up_to_date(proj.base(), parent_key).await {
            Ok(base) => base,
            Err(CycleError { .. }) => return CheckDependencyResult::Changed,
        };
        // A demand recovery may have produced a transient base without changing the core
        // graph. Its old projection certificate is not evidence for this epoch's result.
        if base.revision().is_none() {
            return CheckDependencyResult::Changed;
        }
        if !base.has_resident_value() {
            let validation = eval
                .version_state
                .projection_validation(edge.key)
                .get_or_init(|| validate_projection_without_base(eval, edge.key))
                .await
                .dupe();
            match validation {
                ProjectionValidation::Resolved(projection) => {
                    return compare_revision(&projection, &edge);
                }
                ProjectionValidation::NeedsRecompute => {
                    if !eval
                        .page_in(ValueDemand {
                            key: proj.base(),
                            parent_key,
                        })
                        .await
                        .is_ok_and(|base| base.revision().is_some())
                    {
                        // The parent's recompute must observe the base's read failure or
                        // transient recovery, rather than validate against its old projection.
                        return CheckDependencyResult::Changed;
                    }
                }
            }
        }
    }

    let dep_result = eval.bring_up_to_date(edge.key, parent_key).await;
    match dep_result {
        Ok(dep_result) => compare_revision(dep_result, &edge),
        // The dep is in a cycle with the key being checked, so no value of it is coming. The
        // recompute this leads to runs into the same cycle and reports it.
        Err(CycleError { .. }) => CheckDependencyResult::Changed,
    }
}

fn compare_revision(
    dep_result: &MaybeResidentComputedValue,
    edge: &DepEdge,
) -> CheckDependencyResult {
    // The dep has the recorded revision iff it has the value the compute observed.
    match dep_result.revision() {
        Some(current) if current == edge.revision => {
            CheckDependencyResult::NoChange(dep_result.invalidation_sources())
        }
        _ => CheckDependencyResult::Changed,
    }
}

/// Establishes a projection's result at this version from the core state alone, as the
/// projection's task would without recomputing it.
async fn validate_projection_without_base(
    eval: &TransactionData,
    key: DiceKey,
) -> ProjectionValidation {
    let v = eval.version_state.get_version();
    let state_handle = &eval.dice.state_handle;
    let (candidate, own_sources) = match state_handle
        .lookup_key(VersionedGraphKey::new(v, key))
        .await
    {
        VersionedGraphResult::Match { value } => return ProjectionValidation::Resolved(value),
        VersionedGraphResult::Unknown {
            candidate, sources, ..
        } => (candidate, sources),
    };
    let Some(candidate) = candidate.filter(|c| c.revalidatable) else {
        return ProjectionValidation::NeedsRecompute;
    };
    let cert = candidate.cert.dupe();
    let mut invalidation_sources =
        match check_dependencies(eval, ParentKey::Some(key), &cert.deps).await {
            result @ CheckDependenciesResult::NoChange { .. } => {
                result.unwrap_no_change_invalidation_sources()
            }
            CheckDependenciesResult::NoDeps | CheckDependenciesResult::Changed { .. } => {
                return ProjectionValidation::NeedsRecompute;
            }
        };
    invalidation_sources.update(own_sources);
    let projection = state_handle
        .revalidate(
            VersionedGraphKey::new(v, key),
            eval.storage_type(key),
            candidate,
            invalidation_sources,
        )
        .await;
    ProjectionValidation::Resolved(projection)
}

#[derive(VariantName)]
enum CheckDependenciesResult<'a> {
    NoDeps,
    NoChange {
        invalidation_sources: InvalidationSources,
    },
    Changed {
        /// If any dep has changed, the deps checking doesn't need to be stopped, when something has
        /// changed in a dep in a parallel series we can continue to request and compute the other
        /// paths in that parallel series (and so potentially continue to request new deps).
        ///
        /// Those other checks won't be dropped/cancelled until the continuables future is dropped,
        /// and polling it will continue that deps check process.
        continuables: BoxFuture<'a, ()>,
    },
}
impl CheckDependenciesResult<'_> {
    fn unwrap_no_change_invalidation_sources(self) -> InvalidationSources {
        match self {
            Self::NoChange {
                invalidation_sources,
            } => invalidation_sources,
            _ => panic!(),
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use crate::epoch::worker::CheckDependenciesResult;

    pub(crate) trait CheckDependenciesResultExt {
        fn is_changed(&self) -> bool;
    }

    impl CheckDependenciesResultExt for CheckDependenciesResult<'_> {
        fn is_changed(&self) -> bool {
            match self {
                CheckDependenciesResult::Changed { .. } => true,
                CheckDependenciesResult::NoChange { .. } => false,
                CheckDependenciesResult::NoDeps => false,
            }
        }
    }
}
