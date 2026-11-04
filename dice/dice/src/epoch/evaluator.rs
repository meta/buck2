/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc as StdArc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use derivative::Derivative;
use dice_error::DiceError;
use dice_error::DiceResult;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::is_transient_read_error;
use parking_lot::Mutex;

use crate::ActivationData;
use crate::DiceEvent;
use crate::DynKey;
use crate::api::activation_tracker::PageInPhase;
use crate::api::data::DiceData;
use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::projection::DiceProjectionComputations;
use crate::api::storage_type::StorageType;
use crate::api::user_data::UserComputationData;
use crate::arc::Arc;
use crate::core::graph::revision::EpsilonToken;
use crate::core::graph::revision::Revision;
use crate::core::graph::types::VersionedGraphKey;
use crate::core::state::CoreStateHandle;
use crate::deps::RecordingDepsTracker;
use crate::deps::graph::DepEdge;
use crate::deps::graph::SeriesParallelDeps;
use crate::dice::Dice;
use crate::epoch::cache::ProjectionValidationCell;
use crate::epoch::cache::SharedCache;
use crate::epoch::cache::SharedCacheInsert;
use crate::epoch::cache::SharedCacheLookup;
use crate::epoch::ctx::ComputeCtx;
use crate::epoch::ctx::EvaluationData;
use crate::epoch::ctx::TrackedComputations;
use crate::epoch::task::PreviouslyCancelledTask;
use crate::epoch::task::dice::DiceTask;
use crate::epoch::task::dice::DiceTaskDependedOnByResult;
use crate::epoch::task::dice::PreparedDiceTask;
use crate::epoch::task::projections::ProjectionTaskCompletionHandle;
use crate::epoch::task::promise::DicePromise;
use crate::epoch::worker::DiceTaskWorker;
use crate::epoch::worker::WorkerResult;
use crate::epoch::worker::state::DiceWorkerStateEvaluating;
use crate::epoch::worker::state::DiceWorkerStateFinishedEvaluating;
use crate::key::DiceKey;
use crate::key::DiceKeyErased;
use crate::key::ParentKey;
use crate::value::DiceValidity;
use crate::value::MaybeResidentComputedValue;
use crate::value::MaybeValidDiceValue;
use crate::value::PageInOutcome;
use crate::value::PageInRecovery;
use crate::value::PagedOutValue;
use crate::value::ResidentComputedValueRef;
use crate::versions::VersionNumber;

/// Context that is shared for all current live computations of the same version.
#[derive(Derivative, Dupe, Clone)]
#[derivative(Debug)]
pub(crate) struct VersionState {
    version: VersionNumber,
    #[derivative(Debug = "ignore")]
    cache: SharedCache,
}
enum LookupResult<'d> {
    Finished(&'d MaybeResidentComputedValue),
    Pending(DicePromise<'d>),
    NeedsRestart(PreparedDiceTask<'d>, Option<PreviouslyCancelledTask>),
}

impl VersionState {
    pub(crate) fn new(v: VersionNumber, cache: SharedCache) -> Self {
        Self { version: v, cache }
    }

    fn lookup_entry(&self, key: DiceKey, parent_key: ParentKey) -> LookupResult<'_> {
        let task = match self.cache.get(key) {
            SharedCacheLookup::Finished(result) => return LookupResult::Finished(result),
            SharedCacheLookup::InProgress(task) => task,
            SharedCacheLookup::Vacant => match self.cache.insert(key, parent_key) {
                SharedCacheInsert::Occupied(dice_task) => dice_task,
                SharedCacheInsert::Inserted(prepared_task) => {
                    return LookupResult::NeedsRestart(prepared_task, None);
                }
            },
        };

        match task.depended_on_by(parent_key) {
            DiceTaskDependedOnByResult::Finished(dice_computed_value) => {
                LookupResult::Finished(dice_computed_value)
            }
            DiceTaskDependedOnByResult::Pending(dice_promise) => {
                LookupResult::Pending(dice_promise)
            }
            DiceTaskDependedOnByResult::NeedsRestart(
                prepared_dice_task,
                previously_cancelled_task,
            ) => LookupResult::NeedsRestart(prepared_dice_task, Some(previously_cancelled_task)),
        }
    }

    /// Compute "projection" based on deriving value
    pub(crate) fn compute_projection(
        &self,
        key: DiceKey,
        base: &MaybeValidDiceValue,
        base_revision: Option<Revision>,
        base_invalidation_sources: InvalidationSources,
        transaction: &TransactionData,
    ) -> MaybeResidentComputedValue {
        let task = match self.cache.get_projection(key) {
            SharedCacheLookup::Finished(result) => {
                return result.dupe();
            }
            SharedCacheLookup::InProgress(task) => Err(task),
            SharedCacheLookup::Vacant => match self.cache.insert_projection(key) {
                SharedCacheInsert::Occupied(task) => Err(task.get()),
                SharedCacheInsert::Inserted(new_task) => Ok(new_task),
            },
        };

        match task {
            Ok(handle) => {
                transaction.started(key);
                // We inserted and are expected to do the computation
                let eval_result = transaction.evaluate_projection(
                    key,
                    base,
                    base_revision,
                    base_invalidation_sources,
                );
                let r = handle_project_eval_result(
                    &transaction.dice.state_handle,
                    handle,
                    key,
                    self.version,
                    eval_result,
                );
                transaction.finished(key);
                r
            }
            Err(task) => {
                // Someone else inserted
                task.wait_sync()
            }
        }
    }

    pub(crate) fn get_version(&self) -> VersionNumber {
        self.version
    }

    pub(crate) fn projection_validation(&self, key: DiceKey) -> ProjectionValidationCell {
        self.cache.projection_validation(key)
    }

    pub(crate) fn cache(&self) -> &SharedCache {
        &self.cache
    }
}

/// Evaluates Keys
#[derive(Clone, Dupe)]
pub(crate) struct TransactionData {
    pub(super) version_state: VersionState,
    pub(super) user_data: Arc<UserComputationData>,
    pub(super) dice: StdArc<Dice>,
    pub(super) global_data: StdArc<DiceData>,
}

#[derive(Clone, Dupe)]
pub(crate) struct ValueDemand {
    pub(crate) key: DiceKey,
    pub(crate) parent_key: ParentKey,
}

impl TransactionData {
    pub(crate) fn new(
        version_state: VersionState,
        user_data: Arc<UserComputationData>,
        dice: StdArc<Dice>,
        global_data: StdArc<DiceData>,
    ) -> Self {
        Self {
            version_state,
            user_data,
            dice,
            global_data,
        }
    }

    /// Establish the key's revision without demanding its payload.
    ///
    /// Fails if `key` turns out to be in a dependency cycle with `parent_key`, and only then.
    pub(crate) fn bring_up_to_date(&self, key: DiceKey, parent_key: ParentKey) -> DicePromise<'_> {
        match self.version_state.lookup_entry(key, parent_key) {
            LookupResult::Finished(value) => match value.recovery() {
                Some(recovery) => self.follow_recovery(recovery, ValueDemand { key, parent_key }),
                None => DicePromise::ready(value),
            },
            LookupResult::Pending(promise) => promise,
            LookupResult::NeedsRestart(prepared, previous) => {
                DiceTaskWorker::new(key, self.dupe()).spawn(prepared, previous)
            }
        }
    }

    pub(crate) fn storage_type(&self, key: DiceKey) -> StorageType {
        let key_erased = self.dice.key_index.get(key);
        match key_erased {
            DiceKeyErased::Key(k) => k.storage_type(),
            DiceKeyErased::Projection(p) => p.proj().storage_type(),
        }
    }

    /// Read the payload on demand. If its stored data cannot be read, share a normal
    /// recomputation in this active-version cache. Transient read failures and asserted
    /// values that cannot be recomputed keep the error path.
    pub(crate) async fn page_in(
        &self,
        demand: ValueDemand,
    ) -> DiceResult<ResidentComputedValueRef<'_>> {
        // Look up the descriptor here so recovery can only use this transaction's cache.
        let value = self.bring_up_to_date(demand.key, demand.parent_key).await?;
        let paged_out = match value.try_as_resident() {
            Ok(value) => return Ok(value),
            Err(paged_out) => paged_out,
        };
        let outcome = paged_out
            .outcome
            .get_or_init(|| self.read_or_create_recovery(demand.key, value, paged_out))
            .await;
        // Await recovery outside initialization so every demander registers its own interest.
        self.resolve_page_in(outcome, demand).await
    }

    async fn read_or_create_recovery(
        &self,
        key: DiceKey,
        value: &MaybeResidentComputedValue,
        paged_out: &PagedOutValue,
    ) -> PageInOutcome {
        let storage = self
            .dice
            .pagable_storage
            .as_ref()
            .expect("paged-out values require storage");
        let start = Instant::now();
        let key_dyn = self.dice.key_index.get(key);
        let outcome = match storage.hydrate(key_dyn, paged_out.data_key).await {
            Ok(hydrated) => {
                let resident = hydrated.value.dupe();
                self.dice
                    .state_handle
                    .rehydrate(key, paged_out.data_key, hydrated);
                if let Some(tracker) = self.user_data.activation_tracker.as_ref() {
                    tracker.key_paged_in(
                        DynKey::ref_cast(key_dyn),
                        start,
                        start.elapsed(),
                        PageInPhase::ValueDemand,
                    );
                }
                Ok(value.paged_in(resident))
            }
            Err(error) => {
                self.hydration_failed(key, &error);
                if !is_transient_read_error(&error)
                    && let Some((epsilon, sources)) = self
                        .dice
                        .state_handle
                        .recovery_untracked(VersionedGraphKey::new(
                            self.version_state.get_version(),
                            key,
                        ))
                        .await
                {
                    return PageInOutcome::Recompute(PageInRecovery {
                        task: DiceTask::new_idle(key),
                        epsilon,
                        sources,
                    });
                }
                Err(self.page_in_failed(key, error))
            }
        };
        PageInOutcome::Read(outcome)
    }

    fn follow_recovery<'d>(
        &self,
        recovery: &'d PageInRecovery,
        demand: ValueDemand,
    ) -> DicePromise<'d> {
        match recovery.task.as_ref().depended_on_by(demand.parent_key) {
            DiceTaskDependedOnByResult::Finished(value) => DicePromise::ready(value),
            DiceTaskDependedOnByResult::Pending(promise) => promise,
            DiceTaskDependedOnByResult::NeedsRestart(prepared, previous) => {
                DiceTaskWorker::new_recompute(
                    demand.key,
                    self.dupe(),
                    recovery.epsilon,
                    recovery.sources,
                )
                .spawn(prepared, Some(previous))
            }
        }
    }

    async fn resolve_page_in<'d>(
        &self,
        outcome: &'d PageInOutcome,
        demand: ValueDemand,
    ) -> DiceResult<ResidentComputedValueRef<'d>> {
        match outcome {
            PageInOutcome::Read(Ok(resident)) => Ok(resident.as_ref()),
            PageInOutcome::Read(Err(error)) => Err(error.dupe()),
            PageInOutcome::Recompute(recovery) => Ok(recovered_value(
                self.follow_recovery(recovery, demand).await?,
            )),
        }
    }

    #[cold]
    fn page_in_failed(&self, key: DiceKey, error: anyhow::Error) -> DiceError {
        DiceError::page_in_failed(
            self.dice.key_index.get(key).to_string(),
            error.into_boxed_dyn_error(),
        )
    }

    /// Runs `key`'s computation. `own_sources`, the sources of the key's own dirty in force,
    /// is what the sources of the result start from.
    pub(crate) async fn evaluate(
        &self,
        cancellations: &CancellationContext,
        key: DiceKey,
        state: DiceWorkerStateEvaluating,
        own_sources: InvalidationSources,
    ) -> WorkerResult<DiceWorkerStateFinishedEvaluating> {
        let key_erased = self.dice.key_index.get(key);

        match key_erased {
            DiceKeyErased::Key(key_dyn) => {
                let compute = ComputeCtx {
                    transaction_data: self.dupe(),
                    parent_key: ParentKey::Some(key), // within this key's compute, this key is the parent
                    evaluation_data: Mutex::new(EvaluationData::none()),
                    dependency_failed: AtomicBool::new(false),
                };
                let mut ctx = TrackedComputations::Normal {
                    compute: &compute,
                    dep_trackers: RecordingDepsTracker::new(own_sources),
                }
                .into();

                let value = key_dyn.compute(&mut ctx, cancellations).await;
                let recorded_deps = ctx.0.finalize();
                // A dependency that failed to produce a value is not recorded as a dep, so
                // `deps_validity` cannot reflect its failure.
                let validity = if compute.dependency_failed.into_inner() {
                    DiceValidity::Transient
                } else {
                    recorded_deps.deps_validity
                };

                state.finished(
                    cancellations,
                    KeyEvaluationResult {
                        value: MaybeValidDiceValue::new(value, validity),
                        deps: recorded_deps.deps,
                        storage: key_dyn.storage_type(),
                        invalidation_sources: recorded_deps.invalidation_sources,
                    },
                    compute.evaluation_data.into_inner().into_activation_data(),
                )
            }
            DiceKeyErased::Projection(proj) => {
                // Ending up here is unusual - it means that we have somehow `bring_up_to_date`d a
                // projection key.
                //
                // You'd hope that that's never possible, but unfortunately it is - it happens in
                // dep checks, where we unconditionally `bring_up_to_date` the deps without checking
                // what kind of key they are.
                //
                // Double unfortunately, this is not just a "someone called the wrong function"
                // issue. It's load bearing because the normal projection compute path never
                // does any check-deps like behavior; when a new task is needed, it recomputes the
                // projection instead of consulting the core state for an existing valid value. By
                // going through the `bring_up_to_date` path we get normal dep checking.
                //
                // FIXME(JakobDegen):
                //  1. There's supposed to be an invariant that we only evaluate keys once and this
                //     transparently sets us up to violate that.
                //  2. It's completely unclear why we're ok with this kind of discrepency between
                //     the recompute and normal cases.
                //  3. This is insanity.
                // `check_dependency`, the only caller that brings a projection key up to date,
                // does so only once the base's value is resident.
                //
                // To cycle detection, this task is a node like any other: the dep checks waiting
                // on it are the keys blocked on it, and this wait for the base is its one edge
                // out, which is why the projection key is the parent of the wait. So it can be
                // found in a cycle. When it is, the detector fails the cycle's other waits but
                // never this one: there is nothing to do with an error here, and failing the
                // waits of the cycle's regular keys breaks it all the same, every cycle having
                // one (the base, for a start). This wait then gets whatever value the base
                // finishes with. See `Member::fails` in `epoch::cycles`.
                let base = self
                    .page_in(ValueDemand {
                        key: proj.base(),
                        parent_key: ParentKey::Some(key),
                    })
                    .await
                    .expect(
                        "dependency checks make a projection's base resident first, and cycle \
                         detection never fails a projection key's wait",
                    );
                let result = self.evaluate_projection(
                    key,
                    base.value(),
                    base.revision(),
                    base.invalidation_sources(),
                );

                state.finished(
                    cancellations,
                    result,
                    ActivationData::Evaluated(None), // Projection keys can't set this.
                )
            }
        }
    }

    fn evaluate_projection(
        &self,
        key: DiceKey,
        base: &MaybeValidDiceValue,
        base_revision: Option<Revision>,
        base_invalidation_sources: InvalidationSources,
    ) -> KeyEvaluationResult {
        let DiceKeyErased::Projection(proj) = self.dice.key_index.get(key) else {
            unreachable!("cannot evaluate async keys synchronously")
        };
        let ctx = DiceProjectionComputations {
            data: &self.global_data,
            user_data: &self.user_data,
        };

        let value = proj.proj().compute(base, &ctx);

        KeyEvaluationResult {
            value: MaybeValidDiceValue::new(value, base.validity()),
            deps: SeriesParallelDeps::serial_from_edges(vec![DepEdge::new(
                proj.base(),
                base_revision,
            )]),
            storage: proj.proj().storage_type(),
            invalidation_sources: base_invalidation_sources,
        }
    }

    pub(crate) fn started(&self, k: DiceKey) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data
            .tracker
            .event(DiceEvent::Started { key_type: desc })
    }

    pub(crate) fn finished(&self, k: DiceKey) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data
            .tracker
            .event(DiceEvent::Finished { key_type: desc })
    }

    pub(crate) fn check_deps_started(&self, k: DiceKey) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data
            .tracker
            .event(DiceEvent::CheckDepsStarted { key_type: desc })
    }

    pub(crate) fn check_deps_finished(&self, k: DiceKey) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data
            .tracker
            .event(DiceEvent::CheckDepsFinished { key_type: desc })
    }

    pub(crate) fn compute_started(&self, k: DiceKey) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data
            .tracker
            .event(DiceEvent::ComputeStarted { key_type: desc })
    }

    pub(crate) fn compute_finished(&self, k: DiceKey) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data
            .tracker
            .event(DiceEvent::ComputeFinished { key_type: desc })
    }

    pub(crate) fn hydration_failed(&self, k: DiceKey, error: &anyhow::Error) {
        let desc = self.dice.key_index.get(k).key_type_name();

        self.user_data.tracker.event(DiceEvent::HydrationFailed {
            key_type: desc,
            error: format!("{error:#}"),
            transient: pagable::is_transient_read_error(error),
        })
    }
}

fn handle_project_eval_result(
    state: &CoreStateHandle,
    handle: ProjectionTaskCompletionHandle,
    k: DiceKey,
    v: VersionNumber,
    eval_result: KeyEvaluationResult,
) -> MaybeResidentComputedValue {
    let KeyEvaluationResult {
        value,
        deps,
        storage,
        invalidation_sources,
    } = eval_result;

    handle.compute_finished();

    let result = match value.dupe().into_valid_value() {
        // Blocking here is safe: the core state never waits on compute threads
        // (`compute_finished` above is what makes that true), so the reply arrives after the
        // (bounded) requests ahead of us in its queue are processed.
        Ok(valid_value) => state
            .update_computed_blocking(
                VersionedGraphKey::new(v, k),
                storage,
                valid_value,
                deps.certify(),
                // A projection is computed here without a lookup, so there is no ε from
                // one to stamp; but projection keys are not `Key`s, so they can't be
                // force-dirtied, and their untracked input never leaves its initial
                // revision.
                EpsilonToken::INITIAL,
                invalidation_sources,
            )
            .into_computed(),
        Err(_transient_result) => {
            // transients are never stored in the state, but the result should be shared
            // with async computations as if it were.
            MaybeResidentComputedValue::new_for_transient(value, invalidation_sources)
        }
    };

    handle.complete(result)
}

pub(crate) struct KeyEvaluationResult {
    pub(crate) value: MaybeValidDiceValue,
    pub(crate) deps: SeriesParallelDeps<Option<Revision>>,
    pub(crate) storage: StorageType,
    pub(crate) invalidation_sources: InvalidationSources,
}

fn recovered_value(value: &MaybeResidentComputedValue) -> ResidentComputedValueRef<'_> {
    match value.try_as_resident() {
        Ok(resident) => resident,
        Err(_) => unreachable!("recovery computes a resident value rather than reusing storage"),
    }
}
