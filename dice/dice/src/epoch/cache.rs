/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Shared, concurrent dice task cache that is shared between computations at the same version

use std::fmt;
use std::sync::Arc as StdArc;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use dashmap::DashMap;
use dupe::Dupe;
use lock_free_hashtable::sharded::ShardedLockFreeRawTable;

use crate::arc::Arc;
use crate::arc::ArcBorrow;
use crate::epoch::task::dice::DiceTask;
use crate::epoch::task::dice::DiceTaskInternal;
use crate::epoch::task::dice::DiceTaskRef;
use crate::epoch::task::dice::PreparedDiceTask;
use crate::epoch::task::projections::ProjectionTask;
use crate::epoch::task::projections::ProjectionTaskCompletionHandle;
use crate::key::DiceKey;
use crate::key::ParentKey;
use crate::value::MaybeResidentComputedValue;

/// A projection's result as a dependency check establishes it from the core state alone.
#[derive(Clone, Dupe)]
pub(crate) enum ProjectionValidation {
    /// The projection's current result, matched or revalidated without its base's value.
    Resolved(MaybeResidentComputedValue),
    /// The projection must be recomputed, which needs its base's value.
    NeedsRecompute,
}

pub(crate) type ProjectionValidationCell = StdArc<tokio::sync::OnceCell<ProjectionValidation>>;

#[derive(Allocative)]
struct Data {
    storage: ShardedLockFreeRawTable<Arc<DiceTaskInternal>, 64>,
    projection_storage: ShardedLockFreeRawTable<Arc<ProjectionTask>, 64>,
    /// Shares one validation of each projection among the dependency checks at this version.
    #[allocative(skip)]
    projection_validations: DashMap<DiceKey, ProjectionValidationCell>,
    /// How many tasks have finished with a value, see `completed_tasks`.
    completed_tasks: AtomicU64,
    /// Whether the cycle detector for this cache has been started, see `claim_cycle_detector`.
    cycle_detector_claimed: AtomicBool,
}

#[derive(Allocative, Clone, Dupe)]
pub(crate) struct SharedCache {
    data: StdArc<Data>,
}

impl fmt::Debug for SharedCache {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SharedCache")
    }
}

/// A handle on a `SharedCache` that does not keep it alive. Every worker running in a cache
/// holds the cache, so one that is only held weakly goes away with the last of its workers.
pub(crate) struct WeakSharedCache(Weak<Data>);

impl WeakSharedCache {
    pub(crate) fn upgrade(&self) -> Option<SharedCache> {
        self.0.upgrade().map(|data| SharedCache { data })
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }
}

pub(crate) enum SharedCacheLookup<'d, T> {
    Finished(&'d MaybeResidentComputedValue),
    InProgress(T),
    Vacant,
}

pub(crate) enum SharedCacheInsert<T, N> {
    Occupied(T),
    Inserted(N),
}

impl SharedCache {
    fn key_hash(key: DiceKey) -> u64 {
        (key.index as u64).wrapping_mul(0x9e3779b97f4a7c15)
    }

    pub(crate) fn get(&self, key: DiceKey) -> SharedCacheLookup<'_, DiceTaskRef<'_>> {
        let entry = self
            .data
            .storage
            .lookup(Self::key_hash(key), |task| task.key == key);

        match entry {
            Some(task) => {
                let task = DiceTaskRef { internal: task };
                if let Some(v) = task.get_finished_value() {
                    SharedCacheLookup::Finished(v)
                } else {
                    SharedCacheLookup::InProgress(task)
                }
            }
            None => SharedCacheLookup::Vacant,
        }
    }

    pub(crate) fn get_projection(&self, key: DiceKey) -> SharedCacheLookup<'_, &'_ ProjectionTask> {
        let entry = self
            .data
            .projection_storage
            .lookup(Self::key_hash(key), |task| task.key == key);

        match entry {
            Some(task) => {
                if let Some(v) = task.get().try_read() {
                    SharedCacheLookup::Finished(v)
                } else {
                    SharedCacheLookup::InProgress(task.get())
                }
            }
            None => SharedCacheLookup::Vacant,
        }
    }

    /// Inserts a task for `key` with `parent` as its first dependent, unless one exists.
    pub(crate) fn insert(
        &self,
        key: DiceKey,
        parent: ParentKey,
    ) -> SharedCacheInsert<DiceTaskRef<'_>, PreparedDiceTask<'_>> {
        let maybe_prepared_task = DiceTask::prepare(key, parent, |task| {
            let (entry, not_inserted_value) = self.data.storage.insert(
                Self::key_hash(key),
                task.internal,
                |left, right| left.key == right.key,
                |task| Self::key_hash(task.key),
            );
            let entry = DiceTaskRef { internal: entry };
            match not_inserted_value {
                Some(_) => Err(entry),
                None => Ok(entry),
            }
        });

        match maybe_prepared_task {
            Ok(p) => SharedCacheInsert::Inserted(p),
            Err(t) => SharedCacheInsert::Occupied(t),
        }
    }

    pub(crate) fn insert_projection(
        &self,
        key: DiceKey,
    ) -> SharedCacheInsert<ArcBorrow<'_, ProjectionTask>, ProjectionTaskCompletionHandle> {
        let maybe_prepared_task = ProjectionTask::prepare(key, |task| {
            let (entry, not_inserted_value) = self.data.projection_storage.insert(
                Self::key_hash(key),
                task,
                |left, right| left.key == right.key,
                |task| Self::key_hash(task.key),
            );
            if not_inserted_value.is_some() {
                Err(entry)
            } else {
                Ok(entry.get())
            }
        });

        match maybe_prepared_task {
            Ok(handle) => SharedCacheInsert::Inserted(handle),
            Err(t) => SharedCacheInsert::Occupied(t),
        }
    }

    #[cfg(test)]
    pub(crate) fn testing_insert_task(&self, key: DiceKey, task: DiceTask) {
        let (_, not_inserted_value) = self.data.storage.insert(
            Self::key_hash(key),
            task.internal,
            |left, right| left.key == right.key,
            |task| Self::key_hash(task.key),
        );
        assert!(not_inserted_value.is_none());
    }

    pub(crate) fn projection_validation(&self, key: DiceKey) -> ProjectionValidationCell {
        self.data
            .projection_validations
            .entry(key)
            .or_default()
            .dupe()
    }

    pub(crate) fn new() -> Self {
        SharedCache {
            data: StdArc::new(Data {
                storage: ShardedLockFreeRawTable::new(),
                projection_storage: ShardedLockFreeRawTable::new(),
                projection_validations: DashMap::new(),
                completed_tasks: AtomicU64::new(0),
                cycle_detector_claimed: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn note_task_completed(&self) {
        self.data.completed_tasks.fetch_add(1, Ordering::Relaxed);
    }

    /// How many tasks have finished with a value so far. Only ever grows; two equal readings mean
    /// that no task finished in between.
    pub(crate) fn completed_tasks(&self) -> u64 {
        self.data.completed_tasks.load(Ordering::Relaxed)
    }

    /// Whether the caller is the one to start this cache's cycle detector: true exactly once.
    pub(crate) fn claim_cycle_detector(&self) -> bool {
        !self
            .data
            .cycle_detector_claimed
            .swap(true, Ordering::Relaxed)
    }

    pub(crate) fn downgrade(&self) -> WeakSharedCache {
        WeakSharedCache(StdArc::downgrade(&self.data))
    }

    /// The tasks that may still be running: those whose latest generation has not terminated.
    ///
    /// Callers await the termination of the returned tasks and rely on no `compute` of this cache
    /// running once they are done. Projections are computed synchronously and cannot be awaited,
    /// so the ones in flight are waited for right here instead; there are at most as many as there
    /// are worker threads. Only their `compute`s are waited for, not their values: completing a
    /// projection task requires a response from the core state thread, which is typically the
    /// thread this runs on.
    pub(crate) fn pending_tasks(&self) -> Vec<DiceTask> {
        let mut regular = Vec::new();
        self.for_each_pending_task(|task| regular.push(task.clone_arc()));
        for t in self.data.projection_storage.iter() {
            t.wait_computed();
        }
        regular
    }

    /// Calls `f` with every task whose latest generation has not terminated, the recomputation
    /// of a finished task's unreadable value included (see `PageInRecovery`).
    pub(crate) fn for_each_pending_task(&self, mut f: impl FnMut(DiceTaskRef<'_>)) {
        // A running task may be inserting a dependency right now; this orders the scan after
        // every insert that has begun, so that the new task is seen along with the one that
        // started it.
        self.data.storage.synchronize_with_inserts();

        for entry in self.data.storage.iter() {
            let task = DiceTaskRef { internal: entry };
            // Check the original task first: it may finish and publish a descriptor
            // whose recovery starts while this scan is in progress.
            if task.is_pending() {
                f(task);
            }
            // The ordinary lookup can be finished while its demanded payload is still
            // being recomputed, including while cancellation waits on a critical section.
            if let Some(recovery) = task
                .peek_finished_value()
                .and_then(MaybeResidentComputedValue::recovery)
                && recovery.task.as_ref().is_pending()
            {
                f(recovery.task.as_ref());
            }
        }
    }
}

#[cfg(test)]
impl SharedCache {
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool {
        StdArc::ptr_eq(&self.data, &other.data)
    }
}

pub(crate) mod introspection {
    use crate::epoch::cache::SharedCache;
    use crate::epoch::task::dice::DiceTaskRef;
    use crate::introspection::DiceTaskState;
    use crate::key::DiceKey;

    impl SharedCache {
        pub(crate) fn iter_tasks(&self) -> impl Iterator<Item = (DiceKey, DiceTaskState)> {
            let regular = self.data.storage.iter().map(|entry| {
                let task = DiceTaskRef { internal: entry };
                let task = task
                    .peek_finished_value()
                    .and_then(|value| value.recovery())
                    .map_or(task, |recovery| recovery.task.as_ref());
                (entry.key, task.introspect_state())
            });
            let projection = self
                .data
                .projection_storage
                .iter()
                .map(|entry| (entry.key, entry.introspect_state()));
            regular.chain(projection)
        }
    }
}

#[cfg(test)]
mod tests {
    use allocative::Allocative;
    use async_trait::async_trait;
    use derive_more::Display;
    use dice_futures::cancellation::CancellationContext;
    use dice_futures::spawner::TokioSpawner;
    use futures::FutureExt;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::DiceKeyDyn;
    use crate::api::computations::DiceComputations;
    use crate::api::key::Key;
    use crate::api::key::NoValueSerialize;
    use crate::api::key::ValueSerialize;
    use crate::epoch::cache::SharedCache;
    use crate::epoch::cache::SharedCacheInsert;
    use crate::epoch::cache::SharedCacheLookup;
    use crate::epoch::task::dice::DiceTask;
    use crate::epoch::task::dice::testing_helpers::make_completed_task;
    use crate::epoch::task::promise::DicePromise;
    use crate::epoch::task::spawn_dice_task;
    use crate::key::DiceKey;
    use crate::key::ParentKey;

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

    /// A task that runs for as long as the returned dependent is held.
    fn make_running_task(key: DiceKey) -> (DiceTask, DicePromise<'static>) {
        spawn_dice_task(key, &TokioSpawner, &(), |handle| {
            async move {
                let _handle = handle;
                futures::future::pending().await
            }
            .boxed()
        })
    }

    #[tokio::test]
    async fn pending_tasks_are_those_still_running() {
        let cache = SharedCache::new();

        let completed_key1 = DiceKey { index: 10 };
        let completed_key2 = DiceKey { index: 20 };
        let completed_task1 = make_completed_task::<K>(completed_key1, 1);
        let completed_task2 = make_completed_task::<K>(completed_key2, 2);

        let cancelled_key1 = DiceKey { index: 30 };
        let cancelled_key2 = DiceKey { index: 40 };
        let cancelled_task1 = make_cancelled_task(cancelled_key1).await;
        let cancelled_task2 = make_cancelled_task(cancelled_key2).await;

        let running_key1 = DiceKey { index: 50 };
        let running_key2 = DiceKey { index: 60 };
        let running_key3 = DiceKey { index: 70 };
        let (running_task1, _promise1) = make_running_task(running_key1);
        let (running_task2, _promise2) = make_running_task(running_key2);
        let (running_task3, _promise3) = make_running_task(running_key3);

        cache.testing_insert_task(completed_key1, completed_task1);
        cache.testing_insert_task(completed_key2, completed_task2);
        cache.testing_insert_task(cancelled_key1, cancelled_task1);
        cache.testing_insert_task(cancelled_key2, cancelled_task2);
        cache.testing_insert_task(running_key1, running_task1);
        cache.testing_insert_task(running_key2, running_task2);
        cache.testing_insert_task(running_key3, running_task3);

        assert!(matches!(
            cache.get(completed_key1),
            SharedCacheLookup::Finished(_)
        ));

        let pending_tasks = cache.pending_tasks();

        assert_eq!(pending_tasks.len(), 3);
        // Reporting pending tasks changes nothing about the cache: it keeps accepting work.
        assert!(matches!(
            cache.insert(DiceKey { index: 999 }, ParentKey::None),
            SharedCacheInsert::Inserted(_)
        ));
    }
}
