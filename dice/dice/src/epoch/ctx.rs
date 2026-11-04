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
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc as StdArc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;

use dice_error::DiceError;
use dice_error::DiceResult;
use dice_futures::cancellation::CancellationContext;
use dice_futures::owning_future::OwningFuture;
use dice_futures::spawn::spawn_dropcancel;
use dupe::Dupe;
use futures::FutureExt;
use futures::TryFutureExt;
use futures::future::BoxFuture;
use itertools::Either;
use parking_lot::Mutex;
use pin_project::pin_project;

use crate::ActivationData;
use crate::LinearRecomputeDiceComputations;
use crate::api::computations::DiceComputations;
use crate::api::computations::DiceComputationsData;
use crate::api::data::DiceData;
use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::Key;
use crate::api::projection::ProjectionKey;
use crate::api::user_data::UserComputationData;
use crate::core::graph::revision::Revision;
use crate::deps::LinearDepsTracker;
use crate::deps::RecordedDeps;
use crate::deps::RecordingDepsTracker;
use crate::deps::graph::DepEdge;
use crate::epoch::branches::BranchEntry;
use crate::epoch::branches::LinearRecomputeArena;
use crate::epoch::branches::ParallelArena;
use crate::epoch::branches::ParallelBranchFuture;
use crate::epoch::evaluator::TransactionData;
use crate::epoch::evaluator::ValueDemand;
use crate::epoch::task::promise::DicePromise;
use crate::key::CowDiceKeyHashed;
use crate::key::DiceKey;
use crate::key::ParentKey;
use crate::opaque::OpaqueValue;
use crate::updater::ActiveTransactionGuard;
use crate::value::MaybeResidentComputedValue;
use crate::versions::VersionNumber;

/// Provides a `DiceTransaction` access to key computations.
pub(crate) struct TransactionCtx {
    ctx: ComputeCtx,
    live_version_guard: ActiveTransactionGuard,
}

impl Clone for TransactionCtx {
    fn clone(&self) -> Self {
        Self {
            ctx: ComputeCtx {
                transaction_data: self.ctx.transaction_data.dupe(),
                parent_key: ParentKey::None,
                evaluation_data: Mutex::new(EvaluationData::none()),
                dependency_failed: AtomicBool::new(false),
            },
            live_version_guard: self.live_version_guard.dupe(),
        }
    }
}

impl Dupe for TransactionCtx {}

impl TransactionCtx {
    pub(crate) fn new(
        transaction_data: TransactionData,
        live_version_guard: ActiveTransactionGuard,
    ) -> Self {
        Self {
            ctx: ComputeCtx {
                transaction_data,
                parent_key: ParentKey::None,
                evaluation_data: Mutex::new(EvaluationData::none()),
                dependency_failed: AtomicBool::new(false),
            },
            live_version_guard,
        }
    }

    pub(crate) fn get_version(&self) -> VersionNumber {
        self.ctx.get_version()
    }

    pub(crate) fn compute<'a, K>(
        &'a self,
        key: &K,
    ) -> impl Future<Output = DiceResult<&'a <K as Key>::Value>> + use<'a, K>
    where
        K: Key,
    {
        self.ctx.compute_opaque(key).map_ok(|opaque| {
            opaque
                .derive_from
                .downcast_maybe_transient::<K::Value>()
                .expect("type mismatch")
        })
    }

    pub(crate) fn as_computations(&self) -> TrackedComputations<'_> {
        TrackedComputations::Normal {
            compute: &self.ctx,
            // Provide a dep tracker here because this type expects to track its deps, but in the
            // context of a `DiceTransaction` we don't actually need the data
            dep_trackers: RecordingDepsTracker::new(InvalidationSources::NONE),
        }
    }
}

// Just for convenience
impl Deref for TransactionCtx {
    type Target = ComputeCtx;

    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl<'d> TrackedComputations<'d> {
    /// Gets all the result of the given computation key.
    /// recorded as dependencies of the current computation for which this
    /// context is for.
    pub(crate) fn compute<'a, K>(
        &'a mut self,
        key: &K,
    ) -> impl Future<Output = DiceResult<&'d <K as Key>::Value>> + use<'a, 'd, K>
    where
        K: Key,
    {
        self.ctx_data().compute_opaque(key).map(move |r| {
            r.map(|opaque| Self::opaque_into_value_impl(self.dep_trackers_holder(), opaque))
        })
    }

    /// Like [`Self::compute`], but also returns the key allocation DICE
    /// retains for `key`. Equal keys share one allocation regardless of who
    /// requested them, so this costs an index lookup by position, not a hash.
    pub(crate) fn compute_with_key<'a, K>(
        &'a mut self,
        key: &K,
    ) -> impl Future<Output = DiceResult<(StdArc<K>, &'d <K as Key>::Value)>> + use<'a, 'd, K>
    where
        K: Key,
    {
        self.ctx_data().compute_opaque(key).map(move |r| {
            r.map(|opaque| {
                let key = self.ctx_data().canonical_key::<K>(opaque.derive_from_key);
                (
                    key,
                    Self::opaque_into_value_impl(self.dep_trackers_holder(), opaque),
                )
            })
        })
    }

    /// Compute "opaque" value where the value is only accessible via projections.
    /// Projections allow accessing derived results from the "opaque" value,
    /// where the dependency of reading a projection is the projection value rather
    /// than the entire opaque value.
    pub(crate) fn compute_opaque<'a, K>(
        &'a self,
        key: &K,
    ) -> impl Future<Output = DiceResult<OpaqueValue<'d, K>>> + use<'a, 'd, K>
    where
        K: Key,
    {
        self.ctx_data().compute_opaque(key)
    }

    /// Starts one parallel branch per element, invoking each closure eagerly on a fresh branch
    /// ctx. This is the primitive that all the parallel compute APIs bottom out in; the future
    /// type is generic so that both the boxed (`BoxFuture`) and unboxed (async closure) public
    /// APIs go through here.
    pub(crate) fn compute_many<'a, Starts, G, Fut>(
        &'a mut self,
        starts: Starts,
    ) -> Vec<ParallelBranchFuture<'a, Fut>>
    where
        Starts: IntoIterator<Item = G>,
        Starts::IntoIter: ExactSizeIterator,
        G: FnOnce(&'a mut DiceComputations<'d>) -> Fut,
        Fut: Future,
    {
        let iter = starts.into_iter();
        let mut parallel = self.parallel_builder(iter.len());
        iter.map(|func| parallel.compute(func)).collect()
    }

    pub(crate) fn compute2<'a, G1, Fut1, G2, Fut2>(
        &'a mut self,
        compute1: G1,
        compute2: G2,
    ) -> (
        ParallelBranchFuture<'a, Fut1>,
        ParallelBranchFuture<'a, Fut2>,
    )
    where
        G1: FnOnce(&'a mut DiceComputations<'d>) -> Fut1,
        G2: FnOnce(&'a mut DiceComputations<'d>) -> Fut2,
        Fut1: Future,
        Fut2: Future,
    {
        let mut parallel = self.parallel_builder(2);
        (parallel.compute(compute1), parallel.compute(compute2))
    }

    pub(crate) fn compute3<'a, G1, Fut1, G2, Fut2, G3, Fut3>(
        &'a mut self,
        compute1: G1,
        compute2: G2,
        compute3: G3,
    ) -> (
        ParallelBranchFuture<'a, Fut1>,
        ParallelBranchFuture<'a, Fut2>,
        ParallelBranchFuture<'a, Fut3>,
    )
    where
        G1: FnOnce(&'a mut DiceComputations<'d>) -> Fut1,
        G2: FnOnce(&'a mut DiceComputations<'d>) -> Fut2,
        G3: FnOnce(&'a mut DiceComputations<'d>) -> Fut3,
        Fut1: Future,
        Fut2: Future,
        Fut3: Future,
    {
        let mut parallel = self.parallel_builder(3);

        (
            parallel.compute(compute1),
            parallel.compute(compute2),
            parallel.compute(compute3),
        )
    }

    pub(crate) fn with_linear_recompute<'a, Func, T>(
        &'a mut self,
        func: Func,
    ) -> impl Future<Output = T> + use<'a, 'd, Func, T>
    where
        Func: for<'x> FnOnce(LinearRecomputeDiceComputations<'x, 'a>) -> BoxFuture<'x, T>,
    {
        let (compute_ctx, mut self_dep_trackers) = self.unpack();
        let shared = LinearShared {
            compute_ctx,
            dep_trackers: Mutex::new(LinearDepsTracker::new(
                // TODO(cjhopman): if inspected during the with_linear_recompute, this will be
                // missing some invalidation sources.
                InvalidationSources::NONE,
            )),
            branch_ctxs: LinearRecomputeArena::new(),
        };
        OwningFuture::new(shared, |shared| {
            func(LinearRecomputeDiceComputations(shared))
        })
        .map_taking_data(move |res, shared| {
            let dep_trackers = shared.dep_trackers.into_inner().collect_deps();
            let validity = dep_trackers.deps_validity;
            for edge in dep_trackers.deps.iter_edges() {
                self_dep_trackers.record(edge, validity, InvalidationSources::NONE)
            }
            self_dep_trackers.update_invalidation_sources(dep_trackers.invalidation_sources);
            res
        })
    }

    pub fn spawned<'a, T, Compute>(
        &'a mut self,
        closure: Compute,
    ) -> impl Future<Output = T> + use<'a, 'd, Compute, T>
    where
        T: Send + 'static,
        Compute: (for<'x> FnOnce(
                &'x mut DiceComputations<'_>,
                &'x CancellationContext,
            ) -> BoxFuture<'x, T>)
            + Send
            + 'static,
    {
        let (ctx_data, self_dep_trackers) = self.unpack();
        let inner_core_ctx = ComputeCtx {
            transaction_data: ctx_data.transaction_data.dupe(),
            parent_key: ctx_data.parent_key.dupe(),
            evaluation_data: Mutex::new(EvaluationData::none()),
            dependency_failed: AtomicBool::new(false),
        };

        let user_data = ctx_data.per_transaction_data();
        let spawner = user_data.spawner.dupe();
        let spawner_data = user_data.dupe();

        let task = spawn_dropcancel(
            |cancellation| {
                async move {
                    let mut ctx = TrackedComputations::Normal {
                        compute: &inner_core_ctx,
                        dep_trackers: RecordingDepsTracker::new(InvalidationSources::NONE),
                    }
                    .into();
                    let res = closure(&mut ctx, cancellation).await;
                    let deps = ctx.0.finalize();
                    (res, deps, inner_core_ctx.dependency_failed.into_inner())
                }
                .boxed()
            },
            &*spawner,
            spawner_data,
        );

        task.map(move |(res, deps, dependency_failed)| {
            if dependency_failed {
                ctx_data.record_dependency_failure();
            }
            let validity = deps.deps_validity;
            let mut self_dep_trackers = self_dep_trackers;
            for edge in deps.deps.iter_edges() {
                self_dep_trackers.record(edge, validity, InvalidationSources::NONE)
            }
            self_dep_trackers.update_invalidation_sources(deps.invalidation_sources);

            res
        })
    }

    pub(crate) fn opaque_into_value<K: Key>(&mut self, opaque: OpaqueValue<'d, K>) -> &'d K::Value {
        Self::opaque_into_value_impl(self.unpack().1, opaque)
    }

    fn opaque_into_value_impl<K: Key>(
        mut deps: DepsTrackerHolder<'_, 'd>,
        opaque: OpaqueValue<'d, K>,
    ) -> &'d K::Value {
        let OpaqueValue {
            derive_from_key,
            derive_from,
            revision,
            invalidation_sources,
            ..
        } = opaque;

        deps.record(
            DepEdge::new(derive_from_key, revision),
            derive_from.validity(),
            invalidation_sources,
        );

        derive_from
            .downcast_maybe_transient::<K::Value>()
            .expect("type mismatch")
    }

    pub(crate) fn get_invalidation_sources(&mut self) -> InvalidationSources {
        self.dep_trackers_holder().invalidation_sources()
    }

    pub(crate) fn data(&self) -> DiceComputationsData {
        DiceComputationsData(ModernDiceComputationsData(
            self.ctx_data().transaction_data.dupe(),
        ))
    }
}

impl<'a> From<TrackedComputations<'a>> for DiceComputations<'a> {
    fn from(value: TrackedComputations<'a>) -> Self {
        DiceComputations(value)
    }
}

/// The state shared by all the ctxs of one `with_linear_recompute`.
pub(crate) struct LinearShared<'d> {
    compute_ctx: &'d ComputeCtx,
    dep_trackers: Mutex<LinearDepsTracker>,
    /// Owns the ctx of any parallel futures created from this one.
    branch_ctxs: LinearRecomputeArena,
}

impl LinearShared<'_> {
    pub(crate) fn get(&self) -> DiceComputations<'_> {
        DiceComputations(TrackedComputations::Linear { shared: self })
    }
}

/// This is used to create the ctx for each individual parallel compute (from compute_many/compute_join/compute2/etc).
pub(crate) enum ModernComputeCtxParallelBuilder<'a, 'd> {
    Normal {
        /// The branches, pre-built by `parallel_builder`; `compute` claims them in order.
        handout: std::slice::IterMut<'a, BranchEntry<'d>>,
    },
    Linear {
        shared: &'d LinearShared<'d>,
    },
}

impl<'a, 'd: 'a> ModernComputeCtxParallelBuilder<'a, 'd> {
    fn compute<F, Fut>(&mut self, func: F) -> ParallelBranchFuture<'a, Fut>
    where
        // We don't actually need this closure to be `Send` and so we don't require that here, but
        // all the public APIs still do. It's unclear what we should commit to.
        F: FnOnce(&'a mut DiceComputations<'d>) -> Fut,
        Fut: Future,
    {
        match self {
            ModernComputeCtxParallelBuilder::Normal { handout } => {
                let entry = handout
                    .next()
                    .expect("more branches than the ExactSizeIterator promised");
                ParallelBranchFuture::launch(entry, func)
            }
            ModernComputeCtxParallelBuilder::Linear { shared } => ParallelBranchFuture::launch(
                shared
                    .branch_ctxs
                    .alloc(TrackedComputations::Linear { shared }.into()),
                func,
            ),
        }
    }
}

/// A holder for the user data attached to DICE.
#[derive(Clone, Dupe)]
pub struct ModernDiceComputationsData(TransactionData);

impl ModernDiceComputationsData {
    pub fn global_data(&self) -> &DiceData {
        &self.0.global_data
    }

    pub fn per_transaction_data(&self) -> &UserComputationData {
        &self.0.user_data
    }
}

/// A handle to a `ComputeCtx`, augmented with all the machinery to track the keys that are actually
/// computed as dependencies.
pub(crate) enum TrackedComputations<'a> {
    Normal {
        compute: &'a ComputeCtx,
        dep_trackers: RecordingDepsTracker<'a>,
    },
    /// The ctx within a with_linear_recompute.
    Linear { shared: &'a LinearShared<'a> },
}

impl TrackedComputations<'_> {
    pub(crate) fn finalize(self) -> RecordedDeps {
        match self {
            TrackedComputations::Normal {
                compute: _,
                dep_trackers,
            } => dep_trackers.collect_deps(),
            _ => unreachable!(),
        }
    }
}

/// Write access to whichever dep tracker a ctx records into: A normal ctx's own tracker, or the
/// mutex-shared one of a linear recompute.
struct DepsTrackerHolder<'a, 'd>(
    Either<&'a mut RecordingDepsTracker<'d>, &'a Mutex<LinearDepsTracker>>,
);

impl<'a, 'd> DepsTrackerHolder<'a, 'd> {
    fn record(
        &mut self,
        edge: DepEdge<Option<Revision>>,
        validity: crate::value::DiceValidity,
        invalidation_sources: InvalidationSources,
    ) {
        match &mut self.0 {
            Either::Left(t) => t.record(edge, validity, invalidation_sources),
            Either::Right(m) => m.lock().record(edge, validity, invalidation_sources),
        }
    }

    fn update_invalidation_sources(&mut self, invalidation_sources: InvalidationSources) {
        match &mut self.0 {
            Either::Left(t) => t.update_invalidation_sources(invalidation_sources),
            Either::Right(m) => m.lock().update_invalidation_sources(invalidation_sources),
        }
    }

    fn invalidation_sources(&mut self) -> InvalidationSources {
        match &mut self.0 {
            Either::Left(t) => t.invalidation_sources(),
            Either::Right(m) => m.lock().invalidation_sources(),
        }
    }

    #[cfg(test)]
    pub(crate) fn recorded_deps(&mut self) -> crate::HashSet<DiceKey> {
        use crate::deps::testing::RecordingDepsTrackersExt;
        match &mut self.0 {
            Either::Left(t) => t.recorded_deps(),
            Either::Right(m) => m.lock().recorded_deps(),
        }
    }
}

impl<'d> TrackedComputations<'d> {
    fn parallel_builder<'a>(&'a mut self, len: usize) -> ModernComputeCtxParallelBuilder<'a, 'd> {
        match self {
            TrackedComputations::Normal {
                compute: ctx_data,
                dep_trackers,
            } => {
                let ctx_data = *ctx_data;
                let invalidation_sources = dep_trackers.invalidation_sources();
                let group = ParallelArena::new((0..len).map(|_| {
                    TrackedComputations::Normal {
                        compute: ctx_data,
                        dep_trackers: RecordingDepsTracker::new(invalidation_sources),
                    }
                    .into()
                }));
                ModernComputeCtxParallelBuilder::Normal {
                    handout: dep_trackers.push_parallel(group).handout(),
                }
            }
            TrackedComputations::Linear { shared } => {
                ModernComputeCtxParallelBuilder::Linear { shared }
            }
        }
    }

    pub(super) fn ctx_data(&self) -> &'d ComputeCtx {
        match self {
            TrackedComputations::Normal {
                compute: ctx_data, ..
            } => ctx_data,
            TrackedComputations::Linear { shared, .. } => shared.compute_ctx,
        }
    }

    fn dep_trackers_holder(&mut self) -> DepsTrackerHolder<'_, 'd> {
        self.unpack().1
    }

    fn unpack(&mut self) -> (&'d ComputeCtx, DepsTrackerHolder<'_, 'd>) {
        match self {
            TrackedComputations::Normal {
                compute: ctx_data,
                dep_trackers,
            } => (
                ctx_data,
                DepsTrackerHolder(Either::Left(&mut *dep_trackers)),
            ),
            TrackedComputations::Linear { shared } => (
                shared.compute_ctx,
                DepsTrackerHolder(Either::Right(&shared.dep_trackers)),
            ),
        }
    }

    /// Compute "projection" based on deriving value
    pub(crate) fn projection<K: Key, P: ProjectionKey<DeriveFromKey = K>>(
        &mut self,
        derive_from: &OpaqueValue<K>,
        key: &P,
    ) -> DiceResult<P::Value> {
        let (ctx_data, mut dep_trackers) = self.unpack();
        let (dice_key, res) = ctx_data.project(key, derive_from)?;
        // A projection is computed here and then stored, so its value is always resident.
        let value = res
            .resident_value()
            .expect("a projection's value is never paged out");
        dep_trackers.record(
            DepEdge::new(dice_key, res.revision()),
            value.validity(),
            res.invalidation_sources(),
        );

        Ok(value
            .downcast_maybe_transient::<P::Value>()
            .expect("Type mismatch when computing key")
            .dupe())
    }

    /// Untracked data that is static for the lifetime of this branch tree.
    pub(crate) fn global_data(&self) -> &'d DiceData {
        self.ctx_data().global_data()
    }

    /// Data that is static for the lifetime of the current request context. This lifetime is
    /// the lifetime of the top-level `DiceComputation` used for all requests.
    /// The data is also specific to each request context, so multiple concurrent requests can
    /// each have their own individual data.
    pub(crate) fn per_transaction_data(&self) -> &'d UserComputationData {
        self.ctx_data().per_transaction_data()
    }

    #[cfg(test)]
    pub(crate) fn recorded_deps_for_test(&mut self) -> crate::HashSet<DiceKey> {
        self.unpack().1.recorded_deps()
    }

    pub(crate) fn store_evaluation_data<T: Send + Sync + 'static>(
        &self,
        value: T,
    ) -> DiceResult<()> {
        self.ctx_data().store_evaluation_data(value)
    }
}

/// The base type that provides access to `.compute()` operations and stores the state needed to
/// perform those operations.
///
/// This type does not *track* those dependencies.
pub(crate) struct ComputeCtx {
    pub(crate) transaction_data: TransactionData,
    pub(crate) parent_key: ParentKey,
    // data for the entire compute of a Key, including parallel computes
    pub(crate) evaluation_data: Mutex<EvaluationData>,
    /// Whether a dependency failed to produce a value, for example because its paged-out value
    /// could not be read back. Such a failure reflects storage state rather than the recorded
    /// deps, so a result computed after observing one must not be reused by later transactions.
    pub(crate) dependency_failed: AtomicBool,
}

impl ComputeCtx {
    fn record_dependency_failure(&self) {
        self.dependency_failed.store(true, Ordering::Relaxed);
    }

    /// The allocation the key index holds for an already indexed `key`.
    pub(crate) fn canonical_key<K: Key>(&self, key: DiceKey) -> StdArc<K> {
        self.transaction_data
            .dice
            .key_index
            .get(key)
            .dupe()
            .downcast::<K>()
            .expect("a key indexed as `K` is stored as `K`")
    }

    /// Compute "opaque" value where the value is only accessible via projections.
    /// Projections allow accessing derived results from the "opaque" value,
    /// where the dependency of reading a projection is the projection value rather
    /// than the entire opaque value.
    pub(crate) fn compute_opaque<'d, K>(
        &'d self,
        key: &K,
    ) -> impl Future<Output = DiceResult<OpaqueValue<'d, K>>> + use<'d, K>
    where
        K: Key,
    {
        let dice_key = self
            .transaction_data
            .dice
            .key_index
            .index(CowDiceKeyHashed::key_ref(key));

        let promise = self
            .transaction_data
            .bring_up_to_date(dice_key, self.parent_key);
        ComputeOpaqueFuture {
            state: ComputeOpaqueState::UpToDate {
                promise,
                compute: self,
                key: dice_key,
            },
        }
    }

    fn page_in_opaque<'d, K: Key>(
        &'d self,
        key: DiceKey,
    ) -> BoxFuture<'d, DiceResult<OpaqueValue<'d, K>>> {
        async move {
            let value = self
                .transaction_data
                .page_in(ValueDemand {
                    key,
                    parent_key: self.parent_key,
                })
                .await
                .inspect_err(|_| self.record_dependency_failure())?;
            Ok(OpaqueValue::new(
                key,
                value.value(),
                value.revision(),
                value.invalidation_sources(),
            ))
        }
        .boxed()
    }

    /// Compute "projection" based on deriving value
    pub(super) fn project<B: Key, K: ProjectionKey<DeriveFromKey = B>>(
        &self,
        key: &K,
        base: &OpaqueValue<B>,
    ) -> DiceResult<(DiceKey, MaybeResidentComputedValue)> {
        let dice_key = self
            .transaction_data
            .dice
            .key_index
            .index(CowDiceKeyHashed::proj_ref(base.derive_from_key, key));

        let value = self.transaction_data.version_state.compute_projection(
            dice_key,
            base.derive_from,
            base.revision,
            base.invalidation_sources,
            &self.transaction_data,
        );
        Ok((dice_key, value))
    }

    /// Untracked data that is static for the lifetime of this branch tree.
    pub(crate) fn global_data(&self) -> &DiceData {
        &self.transaction_data.global_data
    }

    /// Data that is static for the lifetime of the current request context. This lifetime is
    /// the lifetime of the top-level `DiceComputation` used for all requests.
    /// The data is also specific to each request context, so multiple concurrent requests can
    /// each have their own individual data.
    pub(crate) fn per_transaction_data(&self) -> &UserComputationData {
        &self.transaction_data.user_data
    }

    pub(crate) fn get_version(&self) -> VersionNumber {
        self.transaction_data.version_state.get_version()
    }

    pub(crate) fn store_evaluation_data<T: Send + Sync + 'static>(
        &self,
        value: T,
    ) -> DiceResult<()> {
        let mut evaluation_data = self.evaluation_data.lock();
        if evaluation_data.0.is_some() {
            return Err(DiceError::duplicate_activation_data());
        }
        evaluation_data.0 = Some(Box::new(value) as _);
        Ok(())
    }
}

#[pin_project(project = ComputeOpaqueStateProj)]
enum ComputeOpaqueState<'d, K: Key> {
    UpToDate {
        #[pin]
        promise: DicePromise<'d>,
        compute: &'d ComputeCtx,
        key: DiceKey,
    },
    PagingIn {
        #[pin]
        future: BoxFuture<'d, DiceResult<OpaqueValue<'d, K>>>,
    },
}

#[pin_project]
struct ComputeOpaqueFuture<'d, K: Key> {
    #[pin]
    state: ComputeOpaqueState<'d, K>,
}

impl<'d, K: Key> Future for ComputeOpaqueFuture<'d, K> {
    type Output = DiceResult<OpaqueValue<'d, K>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        loop {
            let mut this = self.as_mut().project();
            match this.state.as_mut().project() {
                ComputeOpaqueStateProj::UpToDate {
                    promise,
                    compute,
                    key,
                } => {
                    let value = match std::task::ready!(promise.poll(cx)) {
                        Ok(value) => value,
                        Err(cycle) => {
                            // The dep goes unrecorded, as after a failed page-in, and the result
                            // computed without it must likewise not be reused.
                            compute.record_dependency_failure();
                            return Poll::Ready(Err(cycle.into()));
                        }
                    };
                    if let Ok(resident) = value.try_as_resident() {
                        // Return from the poll that produced the resident value; the hot path
                        // needs neither a boxed future nor another future state transition.
                        return Poll::Ready(Ok(OpaqueValue::new(
                            *key,
                            resident.value(),
                            resident.revision(),
                            resident.invalidation_sources(),
                        )));
                    }
                    let future = compute.page_in_opaque(*key);
                    this.state.set(ComputeOpaqueState::PagingIn { future });
                }
                ComputeOpaqueStateProj::PagingIn { future } => return future.poll(cx),
            }
        }
    }
}

/// Opaque data that the key may have provided during evalution via store_evaluation_data.
pub(crate) struct EvaluationData(Option<Box<dyn Any + Send + Sync + 'static>>);

impl EvaluationData {
    pub(crate) fn none() -> Self {
        Self(None)
    }

    pub(crate) fn into_activation_data(self) -> ActivationData {
        ActivationData::Evaluated(self.0)
    }
}
