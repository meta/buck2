/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use dice_core::BranchId;
use pagable::DataKey;

use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::InvalidationSourcePriority;
use crate::api::storage_type::StorageType;
use crate::core::graph::BackedSnapshot;
use crate::core::graph::ComputedValueUpdate;
use crate::core::graph::VersionedGraph;
use crate::core::graph::introspection::VersionedGraphIntrospectable;
use crate::core::graph::revision::EpsilonToken;
use crate::core::graph::types::Candidate;
use crate::core::graph::types::VersionedGraphKey;
use crate::core::graph::types::VersionedGraphResult;
use crate::core::versions::IdleStatus;
use crate::core::versions::VersionTracker;
use crate::core::versions::introspection::VersionIntrospectable;
use crate::dice::PagableNodeCounts;
use crate::epoch::cache::SharedCache;
use crate::epoch::task::dice::DiceTask;
use crate::key::DiceKey;
use crate::metrics::AllocWindow;
use crate::metrics::Metrics;
use crate::metrics::PagingMemoryMetrics;
use crate::storage::EvictionPolicy;
use crate::storage::PagingSetup;
use crate::updater::ChangeType;
use crate::value::DiceValidValue;
use crate::value::HydratedValue;
use crate::value::MaybeResidentComputedValue;
use crate::value::PageOutResult;
use crate::value::ResidentComputedValue;
use crate::versions::VersionNumber;

/// Everything the actor thread owns: the graph and the transactions in flight.
#[derive(allocative::Allocative)]
pub(super) struct ActorState {
    version_tracker: VersionTracker,
    graph: VersionedGraph,
    /// Shared with `DiceStorage`, which measures the page-in side. `None` when
    /// pagable storage is not configured and there is nothing to account for.
    #[allocative(skip)]
    paging_memory: Option<std::sync::Arc<PagingMemoryMetrics>>,
}

/// `ActorState::pagable_status` result. Holds raw `DiceKey`s; the caller resolves
/// them to key types off the core-state thread.
#[derive(Debug)]
pub(crate) struct PagableStatusRaw {
    /// Every key the graph holds anything for, so `>= counts.resident + counts.paged_out` need
    /// not hold: a key may retain several values, and injected keys retain values that are not
    /// counted.
    pub(crate) total_nodes: usize,
    pub(crate) counts: PagableNodeCounts,
    /// Per-key-type breakdown source, one entry per value; lengths equal `counts.resident` /
    /// `counts.paged_out` / `counts.backed`.
    pub(crate) resident: Vec<DiceKey>,
    pub(crate) paged_out: Vec<DiceKey>,
    pub(crate) backed: Vec<DiceKey>,
}

impl ActorState {
    pub(super) fn new(paging: Option<PagingSetup>) -> Self {
        let mut graph = VersionedGraph::new();
        if let Some(paging) = &paging {
            graph.set_page_out_policy(paging.page_out_policy);
        }
        Self {
            version_tracker: VersionTracker::new(),
            graph,
            paging_memory: paging.map(|paging| paging.memory),
        }
    }

    pub(super) fn update_state(
        &mut self,
        branch: BranchId,
        updates: impl IntoIterator<Item = (DiceKey, ChangeType, InvalidationSourcePriority)>,
    ) -> VersionNumber {
        self.graph.commit(branch, updates)
    }

    pub(super) fn ctx_at_version(&mut self, v: VersionNumber) -> SharedCache {
        self.version_tracker.at(v)
    }

    pub(super) fn current_version(&self, branch: BranchId) -> VersionNumber {
        self.graph.head(branch)
    }

    pub(super) fn fork(&mut self, from: VersionNumber) -> BranchId {
        self.graph.fork(from)
    }

    pub(super) fn new_root(&mut self) -> BranchId {
        self.graph.new_root()
    }

    pub(super) fn delete_branch(&mut self, branch: BranchId) {
        self.graph.delete_branch(branch);
    }

    pub(super) fn drop_ctx_at_version(&mut self, v: VersionNumber) {
        self.version_tracker.drop_at_version(v);
    }

    pub(super) fn lookup_key(&mut self, key: VersionedGraphKey) -> VersionedGraphResult {
        self.graph.get(key)
    }

    pub(super) fn recovery_untracked(
        &self,
        key: VersionedGraphKey,
    ) -> Option<(EpsilonToken, InvalidationSources)> {
        self.graph.recovery_untracked(key)
    }

    pub(super) fn update_computed(
        &mut self,
        key: VersionedGraphKey,
        storage: StorageType,
        update: ComputedValueUpdate,
        invalidation_sources: InvalidationSources,
    ) -> ResidentComputedValue {
        self.update_value(storage, |graph| {
            graph.update_computed(key, update, invalidation_sources)
        })
    }

    pub(super) fn revalidate(
        &mut self,
        key: VersionedGraphKey,
        storage: StorageType,
        candidate: Candidate,
        invalidation_sources: InvalidationSources,
    ) -> MaybeResidentComputedValue {
        self.update_value(storage, |graph| {
            graph.revalidate(key, candidate, invalidation_sources)
        })
    }

    fn update_value<T>(
        &mut self,
        storage: StorageType,
        update: impl FnOnce(&mut VersionedGraph) -> T,
    ) -> T {
        if let StorageType::Injected = storage {
            unreachable!(
                "Injected keys should not receive update calls, as those are only from a compute() finishing and InjectedKeys have no compute()"
            );
        }
        update(&mut self.graph)
    }

    pub(super) fn idle_status(&mut self, branch: Option<BranchId>) -> IdleStatus {
        self.version_tracker.idle_status(branch)
    }

    pub(super) fn running_tasks(&mut self, branch: Option<BranchId>) -> Vec<DiceTask> {
        self.version_tracker.running_tasks(branch)
    }

    /// Record the rows of values that still share the exact allocation serialized by page-out.
    /// A recomputation may replace the graph value while serialization runs; its
    /// stale `DataKey` must not evict or back that newer value.
    pub(super) fn commit_page_out(
        &mut self,
        keys: Vec<(DiceKey, PageOutResult)>,
        eviction_policy: EvictionPolicy,
    ) {
        // The graph holds the last reference to each value — `page_out_value`
        // consumed and dropped the worker's copy before this eviction was even
        // queued — so the drops below are where the memory is actually released,
        // and jemalloc charges a free to the thread performing it.
        let window = AllocWindow::open();
        let evicted = self.graph.commit_page_out(keys, eviction_policy);
        if let Some(metrics) = &self.paging_memory {
            metrics.record_nodes_paged_out(evicted);
            metrics.record_offloaded(window.net_freed());
        }
    }

    pub(super) fn backed_snapshot(&self) -> BackedSnapshot {
        self.graph.backed_snapshot()
    }

    pub(super) fn evict_backed(&mut self, keys: &[DiceKey], cutoff: u64) -> u64 {
        let window = AllocWindow::open();
        let evicted = self.graph.evict_backed(keys, cutoff);
        if let Some(metrics) = &self.paging_memory {
            let freed = window.net_freed();
            metrics.record_nodes_paged_out(evicted);
            metrics.record_offloaded(freed);
            metrics.record_evicted(evicted, freed);
        }
        evicted
    }

    /// Mark values that page-out considered but could not serialize, so they are
    /// not offered as page-out candidates again. Ignore stale results if a
    /// recomputation replaced the value while page-out was inspecting it.
    pub(super) fn mark_non_pageable(&mut self, keys: Vec<(DiceKey, DiceValidValue)>) {
        self.graph.mark_non_pageable(keys);
    }

    /// Returns the resident values the next page-out serializes.
    pub(super) fn keys_to_page_out(&self) -> Vec<(DiceKey, DiceValidValue)> {
        self.graph.keys_to_page_out()
    }

    /// Returns the list of `(DiceKey, DataKey)` pairs for every paged-out value. The caller
    /// performs the actual (async) hydration outside the core state thread and sends
    /// rehydrate messages back.
    pub(super) fn paged_out_keys(&self) -> Vec<(DiceKey, DataKey)> {
        self.graph.paged_out_keys()
    }

    /// Classify each computed value as resident (in memory) or paged out (only a `DataKey`
    /// left).
    pub(super) fn pagable_status(&self) -> PagableStatusRaw {
        let (resident, paged_out, backed) = self.graph.keys_by_paging_state();
        let counts = self.graph.pagable_node_counts();
        debug_assert_eq!(resident.len(), counts.resident, "resident count drifted");
        debug_assert_eq!(paged_out.len(), counts.paged_out, "paged-out count drifted");
        debug_assert_eq!(backed.len(), counts.backed, "backed count drifted");
        PagableStatusRaw {
            total_nodes: self.graph.key_count(),
            counts,
            resident,
            paged_out,
            backed,
        }
    }

    pub(super) fn pagable_node_counts(&self) -> PagableNodeCounts {
        self.graph.pagable_node_counts()
    }

    /// Replaces the value of `key` paged out at `data_key` with its hydrated form. No-op if no
    /// such value is retained any more.
    pub(super) fn rehydrate(&mut self, key: DiceKey, data_key: DataKey, hydrated: HydratedValue) {
        self.graph
            .rehydrate(key, data_key, hydrated.value, hydrated.serialized_bytes);
    }

    /// Returns some metrics about the current state of DICE. Don't do expensive things here.
    pub(super) fn metrics(&self) -> Metrics {
        let mut active_transaction_count = 0;

        let currently_active = self.version_tracker.currently_active();
        for active in currently_active {
            active_transaction_count += active.0;
        }

        Metrics {
            key_count: self.graph.key_count(),
            active_transaction_count: active_transaction_count as u32, // probably won't support more than u32 transactions
            branch_count: self.graph.branch_count(),
        }
    }

    pub(super) fn introspection(&self) -> (VersionedGraphIntrospectable, VersionIntrospectable) {
        let graph = self.graph.introspect();
        let version_data = self.version_tracker.introspect();

        (graph, version_data)
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use async_trait::async_trait;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use dice_futures::spawner::TokioSpawner;
    use dupe::Dupe;
    use futures::FutureExt;
    use pagable::Pagable;
    use pagable::pagable_typetag;
    use tokio::sync::Semaphore;

    use crate::DiceKeyDyn;
    use crate::api::computations::DiceComputations;
    use crate::api::invalidation_tracking::InvalidationSources;
    use crate::api::key::InvalidationSourcePriority;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::arc::Arc;
    use crate::core::graph::ComputedValueUpdate;
    use crate::core::graph::revision::EpsilonToken;
    use crate::core::graph::types::VersionedGraphKey;
    use crate::core::internals::ActorState;
    use crate::core::internals::StorageType;
    use crate::deps::graph::SeriesParallelDeps;
    use crate::epoch::cache::SharedCacheInsert;
    use crate::epoch::task::dice::DiceTask;
    use crate::epoch::task::dice::spawn_prepared_task;
    use crate::epoch::task::dice::testing_helpers::make_completed_task;
    use crate::epoch::task::spawn_dice_task;
    use crate::key::DiceKey;
    use crate::key::ParentKey;
    use crate::updater::ChangeType;
    use crate::value::DiceKeyValue;
    use crate::value::DiceValidValue;
    use crate::versions::VersionNumber;

    #[test]
    fn update_state_gets_next_version() {
        let mut core = ActorState::new(None);
        let root = core.new_root();

        assert_eq!(
            core.update_state(
                root,
                [(
                    DiceKey { index: 0 },
                    ChangeType::Invalidate,
                    InvalidationSourcePriority::Normal
                )]
            ),
            VersionNumber::testing_new(2)
        );

        assert_eq!(
            core.update_state(
                root,
                [(
                    DiceKey { index: 1 },
                    ChangeType::Invalidate,
                    InvalidationSourcePriority::Normal
                )]
            ),
            VersionNumber::testing_new(3)
        );
    }

    #[test]
    fn state_ctx_at_version() {
        let mut core = ActorState::new(None);
        let v = VersionNumber::testing_new(1);

        let ctx = core.ctx_at_version(v);

        let ctx1 = core.ctx_at_version(v);
        assert!(ctx.ptr_eq(&ctx1));

        // if you drop one, there is still reference so getting the same version should give the
        // same instance of ctx
        core.drop_ctx_at_version(v);
        let ctx2 = core.ctx_at_version(v);
        assert!(ctx.ptr_eq(&ctx2));

        // drop all references, should give a different ctx instance
        core.drop_ctx_at_version(v);
        core.drop_ctx_at_version(v);
        let another = core.ctx_at_version(v);
        assert!(!ctx.ptr_eq(&another));
    }

    #[test]
    fn non_pageable_nodes_are_not_page_out_candidates() {
        let mut core = ActorState::new(None);
        let root = core.new_root();
        let v = core.current_version(root);
        let _ctx = core.ctx_at_version(v);

        let compute = |core: &mut ActorState, index: u32| {
            core.update_computed(
                VersionedGraphKey::new(v, DiceKey { index }),
                StorageType::Normal,
                ComputedValueUpdate {
                    value: DiceValidValue::testing_new(DiceKeyValue::<K>::new(index as usize)),
                    deps: SeriesParallelDeps::None,
                    epsilon: EpsilonToken::INITIAL,
                },
                InvalidationSources::NONE,
            );
        };
        compute(&mut core, 0);
        compute(&mut core, 1);

        let candidates = |core: &ActorState| {
            let mut keys: Vec<u32> = core
                .keys_to_page_out()
                .into_iter()
                .map(|(k, _)| k.index)
                .collect();
            keys.sort();
            keys
        };

        // Both freshly-computed resident values are page-out candidates.
        assert_eq!(candidates(&core), vec![0, 1]);

        // Marking one non-pageable (its value can't be serialized) drops it from
        // the candidate set, so page-out won't keep retrying it; the other is
        // unaffected.
        let value = core
            .keys_to_page_out()
            .into_iter()
            .find_map(|(key, value)| (key.index == 0).then_some(value))
            .expect("key 0 should be a page-out candidate");
        core.mark_non_pageable(vec![(DiceKey { index: 0 }, value)]);
        assert_eq!(candidates(&core), vec![1]);
    }

    /// A task whose only dependent went away and whose cancellation has landed.
    async fn make_cancelled_task(key: DiceKey) -> DiceTask {
        let (task, promise) = spawn_dice_task(key, &TokioSpawner, &(), |handle| {
            async move {
                let _handle = handle;
                futures::future::pending().await
            }
            .boxed()
        });
        drop(promise);
        task.as_ref().await_termination().await;
        task
    }

    struct BlockCancel(Arc<Semaphore>);

    impl Drop for BlockCancel {
        fn drop(&mut self) {
            self.0.add_permits(1)
        }
    }

    /// A task whose only dependent went away while it sits in a critical section, so that its
    /// cancellation lands once the returned `BlockCancel` is dropped.
    async fn make_blocked_task(key: DiceKey) -> (DiceTask, BlockCancel) {
        let block_cancel = Arc::new(Semaphore::new(0));
        let arrive = Arc::new(Semaphore::new(0));
        let block_cancel_task = block_cancel.dupe();
        let arrive_task = arrive.dupe();
        let (task, promise) = spawn_dice_task(key, &TokioSpawner, &(), move |handle| {
            let block_cancel = block_cancel_task.dupe();
            let arrive = arrive_task.dupe();
            async move {
                handle
                    .cancellation_ctx()
                    .critical_section(|| async move {
                        arrive.add_permits(1);
                        let _guard = block_cancel.acquire().await.unwrap();
                    })
                    .await;
            }
            .boxed()
        });
        arrive.acquire().await.unwrap().forget();
        drop(promise);

        (task, BlockCancel(block_cancel))
    }

    /// A task whose only dependent went away while it sits in a critical section it never leaves.
    async fn make_never_cancellable_task(key: DiceKey) -> DiceTask {
        let arrive = Arc::new(Semaphore::new(0));
        let arrive_task = arrive.dupe();
        let (task, promise) = spawn_dice_task(key, &TokioSpawner, &(), move |handle| {
            let arrive = arrive_task.dupe();
            async move {
                handle
                    .cancellation_ctx()
                    .critical_section(|| async move {
                        arrive.add_permits(1);
                        futures::future::pending().await
                    })
                    .await
            }
            .boxed()
        });
        arrive.acquire().await.unwrap().forget();
        drop(promise);

        task
    }

    #[tokio::test]
    async fn idle_status_tracks_transactions_and_draining_work() {
        let mut core = ActorState::new(None);
        let v = VersionNumber::testing_new(1);

        let cache = core.ctx_at_version(v);

        let completed_key1 = DiceKey { index: 10 };
        let completed_key2 = DiceKey { index: 20 };
        let completed_task1 = make_completed_task::<K>(completed_key1, 1);
        let completed_task2 = make_completed_task::<K>(completed_key2, 2);

        let cancelled_key1 = DiceKey { index: 30 };
        let cancelled_key2 = DiceKey { index: 40 };
        let cancelled_task1 = make_cancelled_task(cancelled_key1).await;
        let cancelled_task2 = make_cancelled_task(cancelled_key2).await;

        let blocked_key1 = DiceKey { index: 50 };
        let blocked_key2 = DiceKey { index: 60 };
        let (blocked_task1, guard1) = make_blocked_task(blocked_key1).await;
        let (blocked_task2, guard2) = make_blocked_task(blocked_key2).await;

        let never_cancel_key1 = DiceKey { index: 100500 };
        let never_cancel_task1 = make_never_cancellable_task(never_cancel_key1).await;

        cache.testing_insert_task(completed_key1, completed_task1);
        cache.testing_insert_task(completed_key2, completed_task2);
        cache.testing_insert_task(cancelled_key1, cancelled_task1);
        cache.testing_insert_task(cancelled_key2, cancelled_task2);
        cache.testing_insert_task(blocked_key1, blocked_task1.dupe());
        cache.testing_insert_task(blocked_key2, blocked_task2.dupe());
        cache.testing_insert_task(never_cancel_key1, never_cancel_task1);

        // Running work counts whether or not a transaction is holding the cache; idleness does
        // not come before the transaction is gone.
        assert_eq!(core.running_tasks(None).len(), 3);
        let status = core.idle_status(None);
        assert_eq!(status.active_transactions, 1);
        assert!(status.draining.is_empty());
        let mut released = status.released.expect("a transaction is alive");

        core.drop_ctx_at_version(v);

        assert!(released.try_recv().is_ok());
        let status = core.idle_status(None);
        assert_eq!(status.active_transactions, 0);
        assert_eq!(status.draining.len(), 3);
        assert!(status.released.is_none());
        assert_eq!(core.running_tasks(None).len(), 3);

        // The cache goes on accepting work from the tasks still running in it.
        let SharedCacheInsert::Inserted(prepared) =
            cache.insert(DiceKey { index: 999 }, ParentKey::None)
        else {
            panic!("the cache should accept new tasks");
        };
        // Run it to termination so that it does not count as pending below.
        let inserted = prepared.task().clone_arc();
        drop(spawn_prepared_task(
            prepared,
            &TokioSpawner,
            &(),
            |_handle| async {}.boxed(),
        ));
        inserted.as_ref().await_termination().await;

        // Let the blocked tasks leave their critical sections, at which point their
        // cancellations land.
        drop(guard1);
        drop(guard2);
        blocked_task1.as_ref().await_termination().await;
        blocked_task2.as_ref().await_termination().await;

        let cache2 = core.ctx_at_version(v);

        let never_cancel_task2 = make_never_cancellable_task(DiceKey { index: 300 }).await;

        cache2.testing_insert_task(DiceKey { index: 300 }, never_cancel_task2);

        core.drop_ctx_at_version(v);

        assert_eq!(core.idle_status(None).draining.len(), 2);

        // Like the workers of a real transaction would, these handles are what keeps the draining
        // caches visible.
        drop((cache, cache2));
        assert!(core.idle_status(None).is_idle());
    }

    #[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct K;

    #[async_trait]
    impl Key for K {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            unimplemented!("test")
        }

        fn equality_behavior() -> crate::EqualityBehavior<Self::Value> {
            crate::EqualityBehavior::Compare(|_, _| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }
}
