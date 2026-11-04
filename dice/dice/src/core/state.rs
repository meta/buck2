/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use dice_core::BranchId;
use dupe::Dupe;
use futures::Future;
use pagable::DataKey;
use static_assertions::const_assert_eq;
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::mpsc::WeakUnboundedSender;
use tokio::sync::oneshot;
use tokio::sync::oneshot::Receiver;
use tokio::sync::oneshot::Sender;

use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::InvalidationSourcePriority;
use crate::api::storage_type::StorageType;
use crate::core::graph::introspection::VersionedGraphIntrospectable;
use crate::core::graph::revision::EpsilonToken;
use crate::core::graph::types::Candidate;
use crate::core::graph::types::VersionedGraphKey;
use crate::core::graph::types::VersionedGraphResult;
use crate::core::internals::ActorState;
use crate::core::internals::PagableStatusRaw;
use crate::core::processor::StateProcessor;
use crate::core::versions::IdleStatus;
use crate::core::versions::introspection::VersionIntrospectable;
use crate::deps::graph::SeriesParallelDeps;
use crate::dice::PagableNodeCounts;
use crate::epoch::evaluator::VersionState;
use crate::epoch::task::dice::DiceTask;
use crate::key::DiceKey;
use crate::metrics::Metrics;
use crate::storage::EvictionPolicy;
use crate::storage::PagingSetup;
use crate::updater::ActiveTransactionGuard;
use crate::updater::ChangeType;
use crate::value::DiceValidValue;
use crate::value::HydratedValue;
use crate::value::MaybeResidentComputedValue;
use crate::value::PageOutResult;
use crate::value::ResidentComputedValue;
use crate::versions::VersionNumber;

/// Padded out to a full cache line so that two of these placed adjacent in a
/// struct sit on different cache lines. 128 bytes covers both 64-byte cache
/// lines and the 128-byte "effective" cache lines created by adjacent-line
/// prefetching on Apple silicon and some Intel processors.
#[repr(align(128))]
struct CachePadded(AtomicUsize);

const_assert_eq!(std::mem::size_of::<CachePadded>(), 128);

/// Two cache-line-separated counters that together track the depth of the request queue feeding the
/// dice core-state thread. The producer side bumps `enqueued`; the (single) consumer side bumps
/// `retired`; depth is the difference.
///
/// Splitting the counter avoids the producer/consumer ping-ponging a single cache line on every
/// send/receive.
pub(super) struct QueueCounters {
    enqueued: CachePadded,
    retired: CachePadded,
}

impl QueueCounters {
    pub(super) fn new() -> Self {
        Self {
            enqueued: CachePadded(AtomicUsize::new(0)),
            retired: CachePadded(AtomicUsize::new(0)),
        }
    }

    fn record_enqueue(&self) {
        self.enqueued.0.fetch_add(1, Ordering::Relaxed);
    }

    pub(super) fn record_retire(&self) {
        self.retired.0.fetch_add(1, Ordering::Relaxed);
    }

    fn approx_depth(&self) -> usize {
        let enqueued = self.enqueued.0.load(Ordering::Relaxed);
        let retired = self.retired.0.load(Ordering::Relaxed);
        // The two relaxed loads can race with concurrent enqueue/retire and
        // momentarily yield retired > enqueued; saturate to 0 in that case.
        enqueued.saturating_sub(retired)
    }

    fn retired_count(&self) -> usize {
        self.retired.0.load(Ordering::Relaxed)
    }
}

/// A handle to the core state that allows sending requests
#[derive(Clone)]
pub(crate) struct CoreStateHandle {
    tx: UnboundedSender<StateRequest>,
    /// Tracks in-flight `StateRequest`s (sent on `tx` but not yet dequeued by
    /// the `StateProcessor`). Used to surface an immediate snapshot of the
    /// queue depth to callers (the canonical metrics request itself runs
    /// through the queue and so could not measure it from inside).
    counters: std::sync::Arc<QueueCounters>,
    // should this handle hold onto the thread and terminate it when all of Dice is dropped?
}

impl fmt::Debug for CoreStateHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CoreStateHandle")
    }
}

impl CoreStateHandle {
    pub(super) fn new(
        tx: UnboundedSender<StateRequest>,
        counters: std::sync::Arc<QueueCounters>,
    ) -> Self {
        Self { tx, counters }
    }

    pub(super) fn downgrade(&self) -> WeakCoreStateHandle {
        WeakCoreStateHandle {
            tx: self.tx.downgrade(),
            counters: self.counters.clone(),
        }
    }

    fn request(&self, message: StateRequest) {
        self.counters.record_enqueue();
        self.tx.send(message).expect("dice runner died");
    }

    /// Current depth of the request queue feeding the dice core-state thread.
    /// Sampled synchronously and may be stale by the time the caller acts on
    /// it, but does not itself enqueue a request.
    pub(crate) fn queue_depth(&self) -> usize {
        self.counters.approx_depth()
    }

    /// Total number of requests the dice core-state thread has dequeued so
    /// far. Monotonic; sampling it at intervals yields the core-state
    /// throughput. Like `queue_depth`, does not itself enqueue a request.
    pub(crate) fn processed_requests(&self) -> usize {
        self.counters.retired_count()
    }

    fn call<T>(
        &self,
        message: StateRequest,
        recv: Receiver<T>,
    ) -> impl Future<Output = T> + use<T> {
        self.request(message);
        futures::FutureExt::map(recv, |v| v.unwrap())
    }

    /// Commits the changes at the branch's head. The new VersionNumber is returned
    pub(crate) fn update_state(
        &self,
        branch: BranchId,
        changes: Vec<(DiceKey, ChangeType, InvalidationSourcePriority)>,
    ) -> impl Future<Output = VersionNumber> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(
            StateRequest::UpdateState {
                branch,
                changes,
                resp,
            },
            recv,
        )
    }

    /// Gets the branch's newest version
    pub(crate) fn current_version(
        &self,
        branch: BranchId,
    ) -> impl Future<Output = VersionNumber> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::CurrentVersion { branch, resp }, recv)
    }

    /// Starts a new branch at `from`. Dropping the future before it yields the branch deletes
    /// it again.
    pub(crate) fn fork(&self, from: VersionNumber) -> impl Future<Output = BranchId> + use<> {
        let (resp, recv) = oneshot::channel();
        futures::FutureExt::map(
            self.call(StateRequest::Fork { from, resp }, recv),
            UnclaimedBranch::claim,
        )
    }

    /// Starts a new branch with no parent. Dropping the future before it yields the branch
    /// deletes it again.
    pub(crate) fn new_root(&self) -> impl Future<Output = BranchId> + use<> {
        let (resp, recv) = oneshot::channel();
        futures::FutureExt::map(
            self.call(StateRequest::NewRoot { resp }, recv),
            UnclaimedBranch::claim,
        )
    }

    /// Deletes a branch. Fire-and-forget; any subsequent state requests are guaranteed to see
    /// the deletion because state requests are processed FIFO.
    pub(crate) fn delete_branch(&self, branch: BranchId) {
        self.request(StateRequest::DeleteBranch { branch })
    }

    /// Obtains the shared state ctx at the given version
    pub(crate) fn ctx_at_version(
        &self,
        version: VersionNumber,
        guard: ActiveTransactionGuard,
    ) -> impl Future<Output = (VersionState, ActiveTransactionGuard)> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(
            StateRequest::CtxAtVersion {
                version,
                guard,
                resp,
            },
            recv,
        )
    }

    /// Report that a computation context at a version has been dropped
    pub(crate) fn drop_ctx_at_version(&self, version: VersionNumber) {
        self.request(StateRequest::DropCtxAtVersion { version })
    }

    /// Lookup the state of a key
    pub(crate) fn lookup_key(
        &self,
        key: VersionedGraphKey,
    ) -> impl Future<Output = VersionedGraphResult> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::LookupKey { key, resp }, recv)
    }

    /// Get the untracked-input revision for demand recovery, or `None` for an asserted key
    /// whose value cannot be replaced by a computation. Only failed demand reads need this.
    pub(crate) fn recovery_epsilon(
        &self,
        key: VersionedGraphKey,
    ) -> impl Future<Output = Option<EpsilonToken>> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::RecoveryEpsilon { key, resp }, recv)
    }

    /// Report that a value has been computed. `epsilon` is the revision of the key's
    /// untracked input that the lookup preceding the computation handed out.
    pub(crate) fn update_computed(
        &self,
        key: VersionedGraphKey,
        storage: StorageType,
        value: DiceValidValue,
        deps: SeriesParallelDeps,
        epsilon: EpsilonToken,
        invalidation_sources: InvalidationSources,
    ) -> impl Future<Output = ResidentComputedValue> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(
            StateRequest::UpdateComputed {
                key,
                storage,
                value,
                deps,
                epsilon,
                invalidation_sources,
                resp,
            },
            recv,
        )
    }

    /// Report that a candidate certificate has been verified to hold at the version: its
    /// ε matched the one the lookup handed out and every dep still has its recorded revision.
    pub(crate) fn revalidate(
        &self,
        key: VersionedGraphKey,
        storage: StorageType,
        candidate: Candidate,
        invalidation_sources: InvalidationSources,
    ) -> impl Future<Output = MaybeResidentComputedValue> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(
            StateRequest::Revalidate {
                key,
                storage,
                candidate,
                resp,
                invalidation_sources,
            },
            recv,
        )
    }

    /// What stands between `branch`, or with `None` everything, and idleness
    pub(crate) fn idle_status(
        &self,
        branch: Option<BranchId>,
    ) -> impl Future<Output = IdleStatus> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::IdleStatus { branch, resp }, recv)
    }

    /// Every task that may still be running, whether its transaction is alive or gone, on
    /// `branch` or, with `None`, anywhere
    pub(crate) fn running_tasks(
        &self,
        branch: Option<BranchId>,
    ) -> impl Future<Output = Vec<DiceTask>> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::RunningTasks { branch, resp }, recv)
    }

    /// Returns the list of `(DiceKey, DataKey)` pairs for paged-out graph nodes.
    /// The caller performs async hydration outside core state and sends
    /// rehydrate messages back.
    pub(crate) fn paged_out_keys(
        &self,
    ) -> impl Future<Output = anyhow::Result<Vec<(DiceKey, DataKey)>>> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::PagedOutKeys { resp }, recv)
    }

    /// Classify graph nodes as resident vs paged out.
    pub(crate) fn pagable_status(&self) -> impl Future<Output = PagableStatusRaw> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::PagableStatus { resp }, recv)
    }

    /// Resident, paged-out, and candidate node counts, read O(1) from the core-state
    /// tallies.
    pub(crate) fn pagable_node_counts(&self) -> impl Future<Output = PagableNodeCounts> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::PagableNodeCounts { resp }, recv)
    }

    /// Returns nodes that need serialization before they can be paged out.
    pub(crate) fn keys_to_page_out(
        &self,
    ) -> impl Future<Output = Vec<(DiceKey, DiceValidValue)>> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::KeysToPageOut { resp }, recv)
    }

    /// Record the rows of values that still match the serialized page-out snapshot, evicting
    /// those never paged out before. Fire-and-forget; any subsequent state requests are
    /// guaranteed to see the result because state requests are processed FIFO.
    pub(crate) fn commit_page_out(
        &self,
        keys: Vec<(DiceKey, PageOutResult)>,
        eviction_policy: EvictionPolicy,
    ) {
        self.request(StateRequest::CommitPageOut {
            keys,
            eviction_policy,
        })
    }

    /// Evict the backed values of up to `limit` keys, returning how many values were evicted:
    /// zero only once none is left.
    pub(crate) fn evict_backed(&self, limit: usize) -> impl Future<Output = u64> + use<> {
        let (resp, recv) = oneshot::channel();
        self.call(StateRequest::EvictBacked { limit, resp }, recv)
    }

    /// Mark nodes that page-out could not serialize so they are not offered as
    /// candidates again. Fire-and-forget (FIFO, as with `commit_page_out`).
    pub(crate) fn mark_non_pageable(&self, keys: Vec<(DiceKey, DiceValidValue)>) {
        self.request(StateRequest::MarkNonPageable { keys })
    }

    /// Replace the value paged out at `data_key` with `value`, its hydrated form.
    /// Fire-and-forget; any subsequent state requests for `key` are guaranteed to see the
    /// hydrated value because state requests are processed FIFO.
    pub(crate) fn rehydrate(&self, key: DiceKey, data_key: DataKey, hydrated: HydratedValue) {
        self.request(StateRequest::Rehydrate {
            key,
            data_key,
            hydrated,
        })
    }

    /// Collect metrics
    pub(crate) fn metrics(&self) -> Metrics {
        let (resp, recv) = oneshot::channel();
        self.request(StateRequest::Metrics { resp });

        // Modern dice can just run on a blocking runtime and block waiting for the channel.
        // This is safe since the processing dice thread is dedicated, and never awaits any other tasks.
        tokio::task::block_in_place(|| recv.blocking_recv().unwrap())
    }

    /// Collects the introspectable dice state
    pub(crate) fn introspection(&self) -> (VersionedGraphIntrospectable, VersionIntrospectable) {
        let (resp, recv) = oneshot::channel();

        self.request(StateRequest::Introspection { resp });

        // Modern dice can just run on a blocking runtime and block waiting for the channel.
        // This is safe since the processing dice thread is dedicated, and never awaits any other tasks.
        tokio::task::block_in_place(|| recv.blocking_recv().unwrap())
    }
}

impl Allocative for CoreStateHandle {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();

        let (resp, recv) = oneshot::channel();
        self.request(StateRequest::MakeAvailableForAllocative { resp });

        let (state, complete_tx) = tokio::task::block_in_place(|| recv.blocking_recv().unwrap());

        // FIXME(JakobDegen): Ideally we'd correctly report the fact that this is shared, but there's
        // no easy identifier to use
        Allocative::visit(&state, &mut visitor);

        drop(state);
        drop(complete_tx);
    }
}

impl Dupe for CoreStateHandle {}

/// Start processing state
pub(crate) fn init_state(paging: Option<PagingSetup>) -> CoreStateHandle {
    StateProcessor::spawn(paging)
}

/// A [`CoreStateHandle`] that does not keep the actor alive: what the actor holds to make
/// requests of itself, since it exits once the last handle is gone.
#[derive(Clone)]
pub(super) struct WeakCoreStateHandle {
    tx: WeakUnboundedSender<StateRequest>,
    counters: std::sync::Arc<QueueCounters>,
}

impl WeakCoreStateHandle {
    /// `None` once every handle is gone.
    fn upgrade(&self) -> Option<CoreStateHandle> {
        Some(CoreStateHandle {
            tx: self.tx.upgrade()?,
            counters: self.counters.clone(),
        })
    }
}

/// A branch the actor has made and no caller has heard of yet: the answer to a `fork` or
/// `new_root`. Dropped unclaimed, which is what happens when the requester stops waiting for the
/// answer, before or after it was sent, it deletes the branch rather than leak it.
pub(super) struct UnclaimedBranch {
    branch: Option<BranchId>,
    actor: WeakCoreStateHandle,
}

impl UnclaimedBranch {
    pub(super) fn new(branch: BranchId, actor: WeakCoreStateHandle) -> Self {
        Self {
            branch: Some(branch),
            actor,
        }
    }

    fn claim(mut self) -> BranchId {
        self.branch.take().expect("claimed once")
    }
}

impl Drop for UnclaimedBranch {
    fn drop(&mut self) {
        // No handle left means the actor is shutting down, branch and all.
        if let Some(branch) = self.branch.take()
            && let Some(actor) = self.actor.upgrade()
        {
            actor.delete_branch(branch);
        }
    }
}

/// Core state is accessed via message passing to a single threaded processor
pub(super) enum StateRequest {
    /// Commits the changes at the branch's head. The new VersionNumber that should be used is
    /// sent back via the channel provided
    UpdateState {
        branch: BranchId,
        changes: Vec<(DiceKey, ChangeType, InvalidationSourcePriority)>,
        resp: Sender<VersionNumber>,
    },
    /// Gets the branch's newest version
    CurrentVersion {
        branch: BranchId,
        resp: Sender<VersionNumber>,
    },
    /// Starts a new branch at `from`
    Fork {
        from: VersionNumber,
        resp: Sender<UnclaimedBranch>,
    },
    /// Starts a new branch with no parent
    NewRoot { resp: Sender<UnclaimedBranch> },
    /// Deletes a branch
    DeleteBranch { branch: BranchId },
    /// Obtains the shared state ctx at the given version
    CtxAtVersion {
        version: VersionNumber,
        guard: ActiveTransactionGuard,
        resp: Sender<(VersionState, ActiveTransactionGuard)>,
    },
    /// Report that a computation context at a version has been dropped
    DropCtxAtVersion { version: VersionNumber },
    /// Lookup the state of a key
    LookupKey {
        key: VersionedGraphKey,
        resp: Sender<VersionedGraphResult>,
    },
    RecoveryEpsilon {
        key: VersionedGraphKey,
        resp: Sender<Option<EpsilonToken>>,
    },
    /// Report that a value has been computed
    UpdateComputed {
        key: VersionedGraphKey,
        /// The storage selection for the key,
        storage: StorageType,
        /// The newly computed value
        value: DiceValidValue,
        /// The deps accessed during the computation of newly computed value
        deps: SeriesParallelDeps,
        /// The revision of the key's untracked input the value was computed under
        epsilon: EpsilonToken,
        invalidation_sources: InvalidationSources,
        /// Response of the new value to use. This could be a different instance that is `Eq` to the
        /// given computed value if the state already stores an instance of value that is equal.
        resp: Sender<ResidentComputedValue>,
    },
    /// Report that a candidate certificate has been verified to hold at the version
    Revalidate {
        key: VersionedGraphKey,
        /// The storage selection for the key,
        storage: StorageType,
        /// The certificate that was verified
        candidate: Candidate,
        invalidation_sources: InvalidationSources,
        /// Response of the new value to use. This could be a different instance that is `Eq` to the
        /// given computed value if the state already stores an instance of value that is equal.
        resp: Sender<MaybeResidentComputedValue>,
    },
    /// What stands between `branch`, or with `None` everything, and idleness
    IdleStatus {
        branch: Option<BranchId>,
        resp: Sender<IdleStatus>,
    },
    /// Every task that may still be running, whether its transaction is alive or gone, on
    /// `branch` or, with `None`, anywhere
    RunningTasks {
        branch: Option<BranchId>,
        resp: Sender<Vec<DiceTask>>,
    },
    /// Collect the keys of all paged-out graph nodes.
    PagedOutKeys {
        resp: Sender<anyhow::Result<Vec<(DiceKey, DataKey)>>>,
    },
    /// Classify graph nodes as resident vs paged out.
    PagableStatus { resp: Sender<PagableStatusRaw> },
    /// Resident, paged-out, and candidate node counts, read O(1) from the core-state
    /// tallies.
    PagableNodeCounts { resp: Sender<PagableNodeCounts> },
    /// Collect nodes that need serialization before they can be paged out.
    KeysToPageOut {
        resp: Sender<Vec<(DiceKey, DiceValidValue)>>,
    },
    /// Record the rows of unchanged page-out snapshots, dropping the in-memory values of those
    /// never paged out before.
    CommitPageOut {
        keys: Vec<(DiceKey, PageOutResult)>,
        eviction_policy: EvictionPolicy,
    },
    /// Drop the backed values of up to `limit` keys, leaving their rows.
    EvictBacked { limit: usize, resp: Sender<u64> },
    /// Mark nodes that page-out could not serialize.
    MarkNonPageable {
        keys: Vec<(DiceKey, DiceValidValue)>,
    },
    /// Replace the value of `key` paged out at `data_key` with its hydrated form.
    Rehydrate {
        key: DiceKey,
        data_key: DataKey,
        hydrated: HydratedValue,
    },
    /// Collect metrics
    Metrics { resp: Sender<Metrics> },
    /// Collects the introspectable dice state
    Introspection {
        resp: Sender<(VersionedGraphIntrospectable, VersionIntrospectable)>,
    },
    /// Makes the dice state available temporarily to be able to run allocative
    ///
    /// Although the `CoreState` is in an `Arc`, this is only to convince the compiler that this is
    /// safe, and it should actually be understood as being a `&'a CoreState`, where `'a` is the
    /// lifetime that starts when the response is sent, and ends when the provided sender is
    /// dropped. Failing to drop all references to the `Arc` by then will cause a panic.
    MakeAvailableForAllocative {
        resp: Sender<(std::sync::Arc<ActorState>, Sender<std::convert::Infallible>)>,
    },
}
