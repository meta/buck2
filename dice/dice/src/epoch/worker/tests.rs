/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice_error::DiceError;
use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::CancellationObserver;
use dupe::Dupe;
use futures::pin_mut;
use itertools::Either;
use pagable::Pagable;
use pagable::PagablePanic;
use pagable::pagable_typetag;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::Semaphore;
use variant_name::VariantName;

use crate::DetectCycles;
use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::data::DiceData;
use crate::api::invalidation_tracking::InvalidationSources;
use crate::api::key::InvalidationSourcePriority;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::api::user_data::UserComputationData;
use crate::arc::Arc;
use crate::core::graph::revision::Revision;
use crate::deps::RecordingDepsTracker;
use crate::deps::graph::DepEdge;
use crate::deps::graph::SeriesParallelDeps;
use crate::dice::Dice;
use crate::epoch::evaluator::TransactionData;
use crate::epoch::task::PreviouslyCancelledTask;
use crate::epoch::task::dice::DiceTask;
use crate::epoch::task::promise::DicePromise;
use crate::epoch::worker::CheckDependenciesResult;
use crate::epoch::worker::DiceTaskWorker;
use crate::epoch::worker::check_dependencies;
use crate::epoch::worker::testing::CheckDependenciesResultExt;
use crate::key::DiceKey;
use crate::key::ParentKey;
use crate::updater::ChangeType;
use crate::value::DiceValidity;
use crate::versions::VersionNumber;

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
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

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Allocative, Clone, Debug, Display, PagablePanic)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct IsRan(Arc<AtomicBool>);

#[async_trait]
impl Key for IsRan {
    type Value = ();

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        self.0.store(true, Ordering::SeqCst);
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

impl PartialEq for IsRan {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}
impl Eq for IsRan {}
impl Hash for IsRan {
    fn hash<H: Hasher>(&self, _state: &mut H) {}
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct Finish;

#[async_trait]
impl Key for Finish {
    type Value = ();

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_, _| true)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

fn spawn_task(
    k: DiceKey,
    eval: TransactionData,
    previously_cancelled_task: Option<PreviouslyCancelledTask>,
) -> (DiceTask, DicePromise<'static>) {
    let prepared_task = DiceTask::prepare_testing(k);
    let task = prepared_task.task().clone_arc();
    let promise = DiceTaskWorker::new(k, eval).spawn(prepared_task, previously_cancelled_task);
    (task, promise)
}

#[tokio::test]
async fn test_detecting_changed_dependencies() -> anyhow::Result<()> {
    let dice = Dice::testing_new(DiceData::new());
    let root = dice.new_root().await;

    let user_data = Arc::new(UserComputationData::new());

    // Set initial value at v0
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(K, 1)]).unwrap();
    updater.commit().await;

    // Change value at v1
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(K, 2)]).unwrap();
    let v1 = updater.commit().await.0.get_version();

    let dep_key = dice.key_index.index_key(K);

    let (ctx, _guard) = dice.testing_shared_ctx(v1).await;
    let eval = TransactionData {
        version_state: ctx.dupe(),
        user_data: user_data.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    // Dep changed between v0 and v1: the edge records the revision K held at v0
    // (`Revision::FIRST`, minted by the initial `changed_to`); K's value moved at v1, so
    // its revision is now a fresh one.
    assert!(
        check_dependencies(
            &eval,
            ParentKey::None,
            &SeriesParallelDeps::serial_from_edges(vec![DepEdge::new(dep_key, Revision::FIRST)]),
        )
        .await
        .is_changed()
    );

    // Advance version without changing dep value
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(K, 2)]).unwrap();
    let v2 = updater.commit().await.0.get_version();

    let (ctx, _guard) = dice.testing_shared_ctx(v2).await;
    let eval = TransactionData {
        version_state: ctx.dupe(),
        user_data: user_data.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    // Dep did NOT change between v1 and v2 (same value): the edge records the revision K
    // held at v1 (the second one minted), which K still holds at v2.
    assert!(
        !check_dependencies(
            &eval,
            ParentKey::None,
            &SeriesParallelDeps::serial_from_edges(vec![DepEdge::new(
                dep_key,
                Revision::testing_new(2),
            )]),
        )
        .await
        .is_changed()
    );

    Ok(())
}

#[tokio::test]
async fn when_equal_return_same_instance() -> anyhow::Result<()> {
    let dice = Dice::testing_new(DiceData::new());
    let root = dice.new_root().await;

    let user_data = Arc::new(UserComputationData::new());

    let instance = Arc::new(AtomicUsize::new(0));

    #[derive(Clone, Dupe, Allocative)]
    struct InstanceEqual {
        instance_count: usize,
    }

    impl PartialEq for InstanceEqual {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    #[derive(Allocative, Clone, Debug, Display, PagablePanic)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct InstanceEqualKey(Arc<AtomicUsize>);

    #[async_trait]
    impl Key for InstanceEqualKey {
        type Value = InstanceEqual;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            InstanceEqual {
                instance_count: self.0.fetch_add(1, Ordering::SeqCst),
            }
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }
    impl PartialEq for InstanceEqualKey {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }
    impl Eq for InstanceEqualKey {}
    impl Hash for InstanceEqualKey {
        fn hash<H: Hasher>(&self, _state: &mut H) {}
    }

    let key = dice.key_index.index_key(InstanceEqualKey(instance.dupe()));

    let v = dice.state_handle.update_state(root, vec![]).await;

    let (ctx, _guard) = dice.testing_shared_ctx(v).await;
    let eval = TransactionData {
        version_state: ctx.dupe(),
        user_data: user_data.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    let (task, _initial_promise) = spawn_task(key.dupe(), eval.dupe(), None);
    let res = task.depended_on_by(ParentKey::None).unwrap().await?;

    let v = dice
        .state_handle
        .update_state(
            root,
            vec![(
                key.dupe(),
                ChangeType::Invalidate,
                InvalidationSourcePriority::Normal,
            )],
        )
        .await;

    let (ctx, _guard) = dice.testing_shared_ctx(v).await;
    let eval = TransactionData {
        version_state: ctx.dupe(),
        user_data: user_data.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    let (task, _initial_promise) = spawn_task(key.dupe(), eval.dupe(), None);
    let res2 = task.depended_on_by(ParentKey::None).unwrap().await?;

    // verify that we incremented the total instance counter
    assert_eq!(instance.load(Ordering::SeqCst), 2);

    // An equal value re-finds its revision.
    assert!(res.revision().is_some(), "a valid value has a revision");
    assert_eq!(res.revision(), res2.revision());

    // verify that the instance we return and store is the same as the original instance
    assert_eq!(
        res.testing_resident_value()
            .downcast_maybe_transient::<InstanceEqual>()
            .unwrap()
            .instance_count,
        res2.testing_resident_value()
            .downcast_maybe_transient::<InstanceEqual>()
            .unwrap()
            .instance_count
    );

    Ok(())
}

#[tokio::test]
async fn spawn_with_no_previously_cancelled_task() {
    let dice = Dice::testing_new(DiceData::new());

    let (shared_ctx, _guard) = dice.testing_shared_ctx(VersionNumber::testing_new(1)).await;

    let is_ran = Arc::new(AtomicBool::new(false));
    let k = dice.key_index.index_key(IsRan(is_ran.dupe()));

    let extra = Arc::new(UserComputationData::new());
    let eval = TransactionData {
        version_state: shared_ctx.dupe(),
        user_data: extra.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };
    let previously_cancelled_task = None;

    let (task, _initial_promise) = spawn_task(k, eval, previously_cancelled_task);

    task.depended_on_by(ParentKey::None).unwrap().await.unwrap();

    assert!(is_ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn spawn_with_previously_cancelled_task_that_cancelled() {
    let dice = Dice::testing_new(DiceData::new());

    let (shared_ctx, _guard) = dice.testing_shared_ctx(VersionNumber::testing_new(1)).await;

    let extra = Arc::new(UserComputationData::new());
    let eval = TransactionData {
        version_state: shared_ctx.dupe(),
        user_data: extra.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    #[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct CancellableNeverFinish;

    #[async_trait]
    impl Key for CancellableNeverFinish {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            futures::future::pending().await
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_, _| unreachable!("test"))
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let k = dice.key_index.index_key(CancellableNeverFinish);
    let (previous_task, prev_task_promise) = spawn_task(k, eval.dupe(), None);

    drop(prev_task_promise);

    let previously_cancelled_task = Some(PreviouslyCancelledTask::new(
        previous_task.as_ref().await_termination(),
    ));

    let is_ran = Arc::new(AtomicBool::new(false));
    let k = dice.key_index.index_key(IsRan(is_ran.dupe()));
    let (task, _initial_promise) = spawn_task(k, eval, previously_cancelled_task);

    task.depended_on_by(ParentKey::None).unwrap().await.unwrap();

    assert!(is_ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn spawn_with_previously_cancelled_task_that_finished() {
    let dice = Dice::testing_new(DiceData::new());

    let (shared_ctx, _guard) = dice.testing_shared_ctx(VersionNumber::testing_new(1)).await;

    let extra = Arc::new(UserComputationData::new());
    let eval = TransactionData {
        version_state: shared_ctx.dupe(),
        user_data: extra.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    #[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Finish;

    #[async_trait]
    impl Key for Finish {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_, _| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let k = dice.key_index.index_key(Finish);
    let (previous_task, prev_task_promise) = spawn_task(k, eval.dupe(), None);
    // wait for it to finish then trigger cancel
    previous_task
        .depended_on_by(ParentKey::None)
        .unwrap()
        .await
        .unwrap();
    drop(prev_task_promise);

    let previously_cancelled_task = Some(PreviouslyCancelledTask::new(
        previous_task.as_ref().await_termination(),
    ));

    let is_ran = Arc::new(AtomicBool::new(false));
    let k = dice.key_index.index_key(IsRan(is_ran.dupe()));
    let (task, _initial_promise) = spawn_task(k, eval, previously_cancelled_task);

    task.depended_on_by(ParentKey::None).unwrap().await.unwrap();

    // The previous generation finished (rather than actually cancelling), so `await_previous` reuses
    // its value instead of recomputing. The new key is therefore never evaluated — this is what
    // avoids a duplicate write of the same value to core state. Contrast with
    // `spawn_with_previously_cancelled_task_that_cancelled`, where the previous task was cancelled
    // and so the new key is recomputed.
    assert!(!is_ran.load(Ordering::SeqCst));
}

/// A worker outliving the last transaction at its version still gets its result stored.
#[tokio::test]
async fn writes_after_the_last_transaction_is_dropped_are_accepted() {
    let dice = Dice::testing_new(DiceData::new());

    let (shared_ctx, guard) = dice.testing_shared_ctx(VersionNumber::testing_new(1)).await;

    let extra = Arc::new(UserComputationData::new());
    let eval = TransactionData {
        version_state: shared_ctx.dupe(),
        user_data: extra.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    drop(guard);

    let k = dice.key_index.index_key(Finish);
    let (task, _initial_promise) = spawn_task(k, eval.dupe(), None);
    task.depended_on_by(ParentKey::None).unwrap().await.unwrap();
}

#[tokio::test]
async fn spawn_with_previously_cancelled_task_nested_cancelled() -> anyhow::Result<()> {
    #[derive(Allocative, Clone, Debug, Display, PagablePanic)]
    #[display("{:?}", self)]
    #[allocative(skip)]
    #[pagable_typetag(DiceKeyDyn)]
    struct DontRunTwice {
        is_started: Arc<Notify>,
        exclusive: Arc<Mutex<bool>>,
        prevent_cancel: Arc<Notify>,
    }

    impl PartialEq for DontRunTwice {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }
    impl Eq for DontRunTwice {}
    impl Hash for DontRunTwice {
        fn hash<H: Hasher>(&self, _state: &mut H) {}
    }

    #[async_trait]
    impl Key for DontRunTwice {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            cancellations: &CancellationContext,
        ) -> Self::Value {
            let mut guard = self
                .exclusive
                .try_lock()
                .expect("Can only have one concurrent execution");

            if *guard {
                // Last attempt, return.
            } else {
                // Note that we did our first execution. Keep the lock held. The point of the
                // test is to prove that nobody will get to run before we exit and drop it.
                *guard = true;

                cancellations
                    .with_structured_cancellation(|obs| async move {
                        // Resume the rest of the code.
                        self.is_started.notify_one();
                        // Wait for our cancellation.
                        obs.await;

                        // Yield. If the final evaluation is ready (that would be a bug!), it will
                        // run now.
                        tokio::task::yield_now().await;
                        self.prevent_cancel.notified().await;
                    })
                    .await;

                // Never return, but this bit will be the one that's cancelled.
                futures::future::pending().await
            }
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let dice = Dice::testing_new(DiceData::new());

    let exclusive = Arc::new(Mutex::new(false));
    let is_started = Arc::new(Notify::new());
    let prevent_cancel = Arc::new(Notify::new());

    let key = DontRunTwice {
        exclusive,
        is_started: is_started.dupe(),
        prevent_cancel: prevent_cancel.dupe(),
    };

    let (shared_ctx, _guard) = dice.testing_shared_ctx(VersionNumber::testing_new(1)).await;

    let k = dice.key_index.index_key(key);

    let extra = Arc::new(UserComputationData::new());
    let eval = TransactionData {
        version_state: shared_ctx.dupe(),
        user_data: extra.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    let (first_task, first_task_promise) = spawn_task(k, eval.dupe(), None);
    is_started.notified().await;
    drop(first_task_promise);

    let (second_task, second_task_promise) = spawn_task(
        k,
        eval.dupe(),
        Some(PreviouslyCancelledTask::new(
            first_task.as_ref().await_termination(),
        )),
    );

    drop(second_task_promise);

    let (third_task, _third_task_promise) = spawn_task(
        k,
        eval,
        Some(PreviouslyCancelledTask::new(
            second_task.as_ref().await_termination(),
        )),
    );

    let promise = third_task.depended_on_by(ParentKey::None).unwrap();

    pin_mut!(promise);

    // if we poll before we allow cancellation, we shouldn't complete
    // tokio doesn't always guarantee the yields switches between tasks so this makes the test
    // slightly more resilient to scheduling
    let res = tokio::time::timeout(Duration::from_secs(5), &mut promise).await;

    assert!(res.is_err());

    prevent_cancel.notify_one();
    let _ignored = promise.await;

    Ok(())
}

#[tokio::test]
async fn test_values_gets_resurrect_if_deps_dont_change_regardless_of_equality()
-> anyhow::Result<()> {
    // A dependency that always recomputes to an equal value.
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Leaf;

    #[async_trait]
    impl Key for Leaf {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            1
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    // Depends on `Leaf`, reports all values as unequal, and counts its own computations. The
    // equality/compute-count split lets the test show that resurrection is driven by deps being
    // unchanged, not by value equality.
    #[derive(
        Clone,
        Dupe,
        Debug,
        Display,
        derivative::Derivative,
        Allocative,
        PagablePanic
    )]
    #[derivative(Hash, PartialEq, Eq)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct NeverEqual(#[derivative(Hash = "ignore", PartialEq = "ignore")] Arc<AtomicUsize>);

    #[async_trait]
    impl Key for NeverEqual {
        type Value = Arc<()>;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            self.0.fetch_add(1, Ordering::SeqCst);
            ctx.compute(&Leaf).await.unwrap();
            Arc::new(())
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| false)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let compute_count = Arc::new(AtomicUsize::new(0));

    // Initial computation: runs `NeverEqual::compute` once and records the dep on `Leaf`.
    let ctx = dice.updater(root).commit().await;
    let initial = ctx.compute(&NeverEqual(compute_count.dupe())).await?;
    assert_eq!(compute_count.load(Ordering::SeqCst), 1);

    // Invalidate the dep. Each round `Leaf` recomputes to an equal value, so `NeverEqual`'s deps
    // are unchanged and its value must be resurrected (reused) rather than recomputed -- even
    // though `NeverEqual::equality` always reports values as different.
    for _ in 0..2 {
        let mut updater = dice.updater(root);
        updater.changed(vec![Leaf])?;
        let ctx = updater.commit().await;

        let resurrected = ctx.compute(&NeverEqual(compute_count.dupe())).await?;

        assert_eq!(
            compute_count.load(Ordering::SeqCst),
            1,
            "NeverEqual must not be recomputed while its deps are unchanged"
        );
        assert!(
            Arc::ptr_eq(&initial, &resurrected),
            "the resurrected value must be the original instance"
        );
    }

    Ok(())
}

// tests that dependency checking stops at the first changed dep in a series node (see SeriesParallelDeps).
//
// it's tough to directly test that, instead we cause the one node we expect to be computed to wait for a
// short period and then check the total number of nodes that get computed.
#[tokio::test]
async fn test_check_dependencies_stops_at_changed() -> anyhow::Result<()> {
    let dice = Dice::testing_new(DiceData::new());
    let root = dice.new_root().await;

    let compute_behavior = (0..20)
        .map(|_v| std::sync::Mutex::new(ComputeBehavior::Immediate))
        .collect();

    let data = Arc::new(Data {
        total_computed: AtomicU32::new(0),
        compute_behavior,
    });

    let spkeys: Vec<_> = (0..20)
        .map(|v| SPKey {
            data: data.clone(),
            idx: v,
        })
        .collect();
    let keys = spkeys
        .iter()
        .map(|v| dice.key_index.index_key(v.dupe()))
        .collect::<Vec<_>>();

    let version = dice.updater(root).commit().await.0.get_version();

    let user_data = Arc::new(UserComputationData::new());
    let (ctx, _guard) = dice.testing_shared_ctx(version).await;

    let eval = TransactionData {
        version_state: ctx.dupe(),
        user_data: user_data.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    *data.compute_behavior[0].lock().unwrap() = ComputeBehavior::Sleep(Duration::from_millis(20));

    // No key has been computed yet, so every dep check computes its key, which mints
    // `Revision::FIRST`. Every edge records a revision no key has: every dep looks changed.
    let deps = SeriesParallelDeps::serial_from_edges(
        keys.iter()
            .map(|k| DepEdge::new(*k, Revision::testing_new(2)))
            .collect(),
    );
    let check_deps_result = check_dependencies(&eval, ParentKey::None, &deps).await;

    match check_deps_result {
        CheckDependenciesResult::Changed { continuables } => {
            continuables.await;
        }
        v => {
            panic!("unexpected checkdeps result {}", v.variant_name())
        }
    }

    assert_eq!(data.total_computed.load(Ordering::SeqCst), 1);

    Ok(())
}

/// tests that dependency checking can continue and fully finish parallel nodes
///
/// we build a series-parallel graph like this:
///
/// ```ignore
///         0
///         1
///         2
///
///  3  |   8   |   13
///  4  |   9   |   14
/// 5 6 | 10 11 | 15 16
///  7  |   12  |   17
///
///        18
///        19
/// ```
///
/// where key 9's value has changed. This should trigger recomputation via check_dependencies in
/// everything except for 10, 11, 12, 18, and 19 (i.e. the 3 and 13 branches should be fully
/// re-computed).
///
/// we block 3 and 13's computation on getting the first result (the DidDepsChange::Changed) back
/// from check_dependencies.
#[tokio::test]
async fn test_check_dependencies_can_eagerly_check_all_parallel_deps() -> anyhow::Result<()> {
    let dice = Dice::testing_new(DiceData::new());
    let root = dice.new_root().await;

    let compute_behavior = (0..20)
        .map(|_v| std::sync::Mutex::new(ComputeBehavior::Immediate))
        .collect();

    let data = Arc::new(Data {
        total_computed: AtomicU32::new(0),
        compute_behavior,
    });

    let spkeys: Vec<_> = (0..20)
        .map(|v| SPKey {
            data: data.clone(),
            idx: v,
        })
        .collect();
    let keys = spkeys
        .iter()
        .map(|v| dice.key_index.index_key(v.dupe()))
        .collect::<Vec<_>>();

    let version = dice.updater(root).commit().await.0.get_version();

    let user_data = Arc::new(UserComputationData::new());
    let (ctx, _guard) = dice.testing_shared_ctx(version).await;

    let eval = TransactionData {
        version_state: ctx.dupe(),
        user_data: user_data.dupe(),
        global_data: dice.global_data.dupe(),
        dice: dice.dupe(),
    };

    let semaphore = Arc::new(Semaphore::new(0));

    *data.compute_behavior[3].lock().unwrap() = ComputeBehavior::WaitFor(semaphore.dupe());
    *data.compute_behavior[13].lock().unwrap() = ComputeBehavior::WaitFor(semaphore.dupe());

    // No key has been computed yet, so every dep check computes its key, which mints
    // `Revision::FIRST`. Every edge records that revision except key 9's, which records one
    // no key has: only key 9 looks changed.
    let mut deps = RecordingDepsTracker::new(InvalidationSources::NONE);
    let record = |deps: &mut RecordingDepsTracker<'_>, k: DiceKey| {
        let recorded = if k == keys[9] {
            Revision::testing_new(2)
        } else {
            Revision::FIRST
        };
        deps.record(
            DepEdge::new(k, Some(recorded)),
            DiceValidity::Valid,
            InvalidationSources::NONE,
        );
    };
    record(&mut deps, keys[0]);
    record(&mut deps, keys[1]);
    record(&mut deps, keys[2]);
    {
        let mut branches = Vec::new();
        for i in 0..3 {
            let offset = i * 5;
            let mut deps = RecordingDepsTracker::new(InvalidationSources::NONE);
            record(&mut deps, keys[3 + offset]);
            record(&mut deps, keys[4 + offset]);
            {
                let mut inner_branches = Vec::new();
                let mut inner = RecordingDepsTracker::new(InvalidationSources::NONE);
                record(&mut inner, keys[5 + offset]);
                inner_branches.push(inner.collect_deps());
                let mut inner = RecordingDepsTracker::new(InvalidationSources::NONE);
                record(&mut inner, keys[6 + offset]);
                inner_branches.push(inner.collect_deps());
                deps.insert_parallel_for_test(inner_branches);
            }
            record(&mut deps, keys[7 + offset]);
            branches.push(deps.collect_deps());
        }
        deps.insert_parallel_for_test(branches);
    }
    record(&mut deps, keys[18]);
    record(&mut deps, keys[19]);

    let deps = deps.collect_deps().deps.certify();

    let check_deps_result = check_dependencies(&eval, ParentKey::None, &deps).await;

    match check_deps_result {
        CheckDependenciesResult::Changed { continuables } => {
            semaphore.add_permits(100);
            continuables.await;
        }
        v => {
            panic!("unexpected checkdeps result {}", v.variant_name())
        }
    }

    assert_eq!(data.total_computed.load(Ordering::SeqCst), 15);

    Ok(())
}

/// After a dep's value changes and the dependent recomputes to an equal value (a
/// reuse-write), a later dep check must not force the dependent to recompute again: the
/// reuse-write has to refresh the stored edges to the dep revisions it observed, or the
/// recorded revision stays pinned at the pre-change value and every later CheckDeps
/// reports it changed.
///
/// `Middle` is a non-injected key so it can be force-dirtied without changing its value
/// in round 3; `Base` is the injected value moved to drive `Middle`'s revision forward.
#[tokio::test]
async fn reuse_write_refreshes_stored_dep_revisions() -> anyhow::Result<()> {
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Base;

    #[async_trait]
    impl crate::InjectedKey for Base {
        type Value = u32;

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    // Middle: relays Base's value. Non-injected so it can be force-dirtied.
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Middle;

    #[async_trait]
    impl Key for Middle {
        type Value = u32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            *ctx.compute(&Base).await.unwrap()
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    // Dependent's compute returns a constant, so a dep-change followed by
    // recompute always yields an equal value - hits the EqualityBased reuse
    // path exactly.
    #[derive(
        Clone,
        Dupe,
        Debug,
        Display,
        derivative::Derivative,
        Allocative,
        PagablePanic
    )]
    #[derivative(Hash, PartialEq, Eq)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Dependent(#[derivative(Hash = "ignore", PartialEq = "ignore")] Arc<AtomicUsize>);

    #[async_trait]
    impl Key for Dependent {
        type Value = u32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            self.0.fetch_add(1, Ordering::SeqCst);
            let _ = *ctx.compute(&Middle).await.unwrap();
            0 // constant — equal every time
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let counter = Arc::new(AtomicUsize::new(0));

    // Round 1: initial compute. Middle=10 with revision R1; Dependent records
    // edge (Middle, R1); counter=1.
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Base, 10)])?;
    let ctx = updater.commit().await;
    let _ = ctx.compute(&Dependent(counter.dupe())).await?;
    assert_eq!(counter.load(Ordering::SeqCst), 1);

    // Round 2: move Base to 20. Middle's dep-check fails, Middle recomputes
    // to 20 (mint R2). Dependent's dep-check on Middle fails (R1 != R2),
    // Dependent recomputes to 0 (equal). Reuse-write fires: WITH FIX the
    // stored edge becomes (Middle, R2); WITHOUT FIX it stays (Middle, R1).
    // counter=2 either way.
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Base, 20)])?;
    let ctx = updater.commit().await;
    let _ = ctx.compute(&Dependent(counter.dupe())).await?;
    assert_eq!(counter.load(Ordering::SeqCst), 2);

    // Round 3: force-dirty Middle WITHOUT moving Base. Middle recomputes: 20
    // == stored 20 => intern reuses R2. Dep-check on Dependent: recorded
    // vs Middle's current R2. WITH FIX: recorded is R2 => match => NoChange
    // => counter stays 2. WITHOUT FIX: recorded is stale R1 => mismatch =>
    // Changed => Dependent recomputes => counter=3.
    let mut updater = dice.updater(root);
    updater.changed(vec![Middle])?;
    let ctx = updater.commit().await;
    let _ = ctx.compute(&Dependent(counter.dupe())).await?;
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "round 2's reuse-write must refresh the stored edge revision so that round 3's \
         dep check revalidates by revision equality"
    );

    Ok(())
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq)]
enum CancelType {
    NotCancelled,
    ASync,
    Sync,
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq)]
enum CancelPollerType {
    Context,
    Observer,
}

async fn _run_cancellation_caching_test(
    name: &'static str,
    cancel_type: CancelType,
    expected_value_from_cache: Result<CancelType, DiceError>,
    cancel_poller_type: CancelPollerType,
) {
    #[derive(Debug)]
    struct KeySyncData {
        cancel_type: CancelType,
        duration_ms: u64,
        ran_compute: bool,
        cancel_poller_type: CancelPollerType,
    }

    #[derive(Allocative, Clone, Dupe, Debug, Display, PagablePanic)]
    #[display("DelayOrCancelsSynchronously<{}>", name)]
    #[pagable_typetag(DiceKeyDyn)]
    struct DelayOrCancelsSynchronously {
        name: &'static str,
        // N.B. The Arcs here are used to share state among keys that are stored
        // in the cache and keys that we have in the test. Once a particular key
        // is given to dice, any equal keys will need to use the same Arc
        // pointers to influence/affect the dice-cached key.
        #[allocative(skip)]
        semaphore: Arc<Semaphore>,
        #[allocative(skip)]
        shared: Arc<Mutex<KeySyncData>>,
        /// Whether a synchronous loop returned because it saw the cancellation.
        /// Atomic: the loop runs on a runtime thread, where the mutex above
        /// cannot be locked synchronously.
        #[allocative(skip)]
        saw_cancellation: Arc<AtomicBool>,
    }

    impl Eq for DelayOrCancelsSynchronously {}
    impl PartialEq for DelayOrCancelsSynchronously {
        fn eq(&self, other: &Self) -> bool {
            self.name == other.name
        }
    }

    impl Hash for DelayOrCancelsSynchronously {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.name.hash(state)
        }
    }

    impl DelayOrCancelsSynchronously {
        async fn reset(
            &self,
            cancel_type: &CancelType,
            duration_ms: u64,
            cancel_poller_type: CancelPollerType,
        ) {
            let mut shared: tokio::sync::MutexGuard<'_, KeySyncData> = self.shared.lock().await;
            shared.cancel_type = cancel_type.dupe();
            shared.duration_ms = duration_ms;
            shared.ran_compute = false;
            shared.cancel_poller_type = cancel_poller_type;
            self.saw_cancellation.store(false, Ordering::Release);

            self.semaphore.forget_permits(1);
        }
    }

    #[async_trait]
    impl Key for DelayOrCancelsSynchronously {
        type Value = CancelType;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            cancellations: &CancellationContext,
        ) -> Self::Value {
            let (cancel_type, duration_ms, cancel_poller_type) = {
                let mut shared: tokio::sync::MutexGuard<'_, KeySyncData> = self.shared.lock().await;
                shared.ran_compute = true;
                (
                    shared.cancel_type.dupe(),
                    shared.duration_ms,
                    shared.cancel_poller_type.dupe(),
                )
            };
            let start = Instant::now();

            let duration = Duration::from_millis(duration_ms);

            // This helper loop is used for "synchronous" cancellation tests. We
            // loop infinitely in here until either the duration expires, or we
            // detect that we are cancelled (via the requested method).
            let sync_loop = |cancel_type,
                             poller: Either<&CancellationContext, &CancellationObserver>|
             -> CancelType {
                while duration > Instant::now() - start {
                    if match cancel_type {
                        CancelType::NotCancelled => false,
                        CancelType::ASync => {
                            panic!("sychronous loop tried to detect async cancellation");
                        }
                        CancelType::Sync => match poller {
                            Either::Left(context) => context.is_cancelled(),
                            Either::Right(observer) => observer.is_cancelled(),
                        },
                    } {
                        self.saw_cancellation.store(true, Ordering::Release);
                        return cancel_type;
                    }
                    // Several of these tests spin at once under `cargo test`
                    // (it runs tests in parallel threads, Buck one per
                    // process); don't starve the runtime threads that
                    // deliver the cancellation on a small machine.
                    std::thread::yield_now();
                }
                CancelType::NotCancelled
            };

            // Run the test with either the CancellationContext or the CancellationObserver
            match cancel_poller_type {
                CancelPollerType::Observer => {
                    cancellations
                        .with_structured_cancellation(async |observer| {
                            // N.B. The point of the semaphore here is to guarantee
                            // to the test case that we've reached the point where
                            // the CancellationObserver has been created. If
                            // _anything_ cancels this task before this is called,
                            // the test case will hang waiting for the semaphore to
                            // be acquired.
                            //
                            // The test itself doesn't drop the task handle (causing
                            // cancellation) until the semaphore is acquired, so
                            // this _should_ be safe, unless there's another
                            // potential source of task cancellations.
                            //
                            // If that becomes the case in the future, the test can
                            // add a timeout at the acquire, and fail at the timeout.
                            self.semaphore.add_permits(1);
                            if cancel_type == CancelType::ASync {
                                // async cancellation must perform at least one
                                // await in the loop, vs synchronous cancellation,
                                // so it's a different worker loop than the
                                // synchronous version in `sync_loop`.
                                let delay = tokio::time::sleep(duration);
                                futures::pin_mut!(delay);
                                futures::pin_mut!(observer);
                                match futures::future::select(delay, observer).await {
                                    futures::future::Either::Left(_) => CancelType::NotCancelled,
                                    futures::future::Either::Right(_) => cancel_type,
                                }
                            } else {
                                sync_loop(cancel_type.dupe(), Either::Right(&observer))
                            }
                        })
                        .await
                }
                CancelPollerType::Context => {
                    self.semaphore.add_permits(1);
                    sync_loop(cancel_type.dupe(), Either::Left(cancellations))
                }
            }
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_, _| unreachable!("test"))
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    const FIRST_COMPUTE_MS: u64 = 2500;
    const CANCELLATION_WAIT_MS: u64 = 500;

    let semaphore = Arc::new(Semaphore::new(1));
    let shared = Arc::new(Mutex::new(KeySyncData {
        cancel_type: CancelType::NotCancelled,
        duration_ms: 0,
        ran_compute: false,
        cancel_poller_type: CancelPollerType::Context,
    }));
    let saw_cancellation = Arc::new(AtomicBool::new(false));

    let k = DelayOrCancelsSynchronously {
        name,
        shared: shared.dupe(),
        semaphore: semaphore.dupe(),
        saw_cancellation: saw_cancellation.dupe(),
    };
    k.reset(&cancel_type, FIRST_COMPUTE_MS, cancel_poller_type)
        .await;
    let txn: crate::DiceTransaction = dice.updater(root).commit().await;
    let task = txn.compute(&k);
    let start_cancel = {
        // Before cancelling the task, we have to wait for the `compute()` method on
        // the key to start, otherwise we may cancel "early" in an await before the
        // key computation begins.
        let _permit = semaphore.acquire().await.unwrap();
        let start = Instant::now();
        // Cancel the computation by dropping the handle to the computation task
        drop(task);
        // Wait for the task to finish it's cancellation before dropping the
        // transaction.  We can't use dice.wait_for_idle() here, because that
        // only tracks that tasks that were cancelled due to dropping a
        // transaction have finished cancelling. We're dropping the task itself,
        // but not the transaction.
        //
        // Failing to wait here races the computation completing and (maybe)
        // returning a value, and dice determining that there are no outstanding
        // transactions and bumping the ??epoch version/minor version?? number.
        // If the latter happens first (it does in practice), when the
        // computation finishes the worker will attempt to write the value, but
        // it will do so for the previous dice minor version, and so fail to
        // actually store the value.
        std::thread::sleep(Duration::from_millis(CANCELLATION_WAIT_MS));
        start
    };
    // Note that we can't check an expected_value_initial_compute here because
    // we drop the task handle, preventing us from awaiting on it for a result.
    assert!(shared.lock().await.ran_compute);
    if cancel_type == CancelType::Sync {
        assert!(
            saw_cancellation.load(Ordering::Acquire),
            "the synchronous loop did not see the cancellation within {CANCELLATION_WAIT_MS}ms"
        );
    }

    // Now in the same transaction, compute the key again, but this time with a
    // key that should succeed. Since we don't cancel this computation it
    // doesn't matter which CancelPollerType is used.
    k.reset(&CancelType::NotCancelled, 0, CancelPollerType::Context)
        .await;
    let myresult = txn.compute(&k).await;
    let elapsed_ms = (Instant::now() - start_cancel).as_millis() as u64;
    // Ensure that the time it took to run the test is expected
    match cancel_type {
        CancelType::Sync | CancelType::ASync => {
            let limit = (FIRST_COMPUTE_MS as f64) * 0.5 + (CANCELLATION_WAIT_MS as f64) * 1.05;
            assert!(
                elapsed_ms < limit as u64,
                "cancelled key recompute took too long: elapsed_ms={elapsed_ms} but expected < {limit_u64} \
                 (limit={limit:.1}, cancel_type={cancel_type:?}, FIRST_COMPUTE_MS={FIRST_COMPUTE_MS}, CANCELLATION_WAIT_MS={CANCELLATION_WAIT_MS})",
                limit_u64 = limit as u64,
            );
        }
        CancelType::NotCancelled => {
            let limit = (std::cmp::min(FIRST_COMPUTE_MS, CANCELLATION_WAIT_MS) as f64) * 0.95;
            assert!(
                elapsed_ms > limit as u64,
                "uncancelled key compute finished too fast: elapsed_ms={elapsed_ms} but expected > {limit_u64} \
                 (limit={limit:.1}, cancel_type={cancel_type:?}, FIRST_COMPUTE_MS={FIRST_COMPUTE_MS}, CANCELLATION_WAIT_MS={CANCELLATION_WAIT_MS})",
                limit_u64 = limit as u64,
            );
        }
    };
    let ran_compute = shared.lock().await.ran_compute;
    // Once tasks are cancelled, they can't end up in the cache. The second compute should always run.
    assert!(ran_compute);
    assert!(myresult.is_ok() == expected_value_from_cache.is_ok());
    assert!(myresult.is_err() || (*myresult.unwrap() == expected_value_from_cache.unwrap()));
}

macro_rules! cancellation_caching_test {
    ($name:ident, $cancel_type:expr, $expected_value_from_cache:expr, $cancel_poller_type:expr) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
        async fn $name() {
            _run_cancellation_caching_test(
                stringify!($name),
                $cancel_type,
                $expected_value_from_cache,
                $cancel_poller_type,
            )
            .await;
        }
    };
}

cancellation_caching_test!(
    test_cancellation_sync_correct_context_does_not_cache,
    CancelType::Sync,
    Ok(CancelType::NotCancelled),
    CancelPollerType::Context
);
cancellation_caching_test!(
    test_cancellation_sync_correct_observer_does_not_cache,
    CancelType::Sync,
    Ok(CancelType::NotCancelled),
    CancelPollerType::Observer
);
cancellation_caching_test!(
    test_cancellation_non_cancelled_context_caches_good_value,
    CancelType::NotCancelled,
    Ok(CancelType::NotCancelled),
    CancelPollerType::Context
);
cancellation_caching_test!(
    test_cancellation_non_cancelled_observer_caches_good_value,
    CancelType::NotCancelled,
    Ok(CancelType::NotCancelled),
    CancelPollerType::Observer
);
cancellation_caching_test!(
    test_cancellation_async_correct_does_not_cache,
    CancelType::ASync,
    Ok(CancelType::NotCancelled),
    // Note there is no test for ASync+CancelPollerType::Context, that's not possible/supported.
    CancelPollerType::Observer
);

#[derive(Clone, Debug)]
enum ComputeBehavior {
    Sleep(Duration),
    WaitFor(Arc<Semaphore>),
    Immediate,
}

#[derive(Debug)]
struct Data {
    total_computed: AtomicU32,
    compute_behavior: Vec<std::sync::Mutex<ComputeBehavior>>,
}

#[derive(Allocative, Clone, Dupe, Debug, PagablePanic)]
#[pagable_typetag(DiceKeyDyn)]
struct SPKey {
    #[allocative(skip)]
    data: Arc<Data>,
    idx: usize,
}

impl Display for SPKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SPKey({})", self.idx)
    }
}
impl Hash for SPKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.idx.hash(state);
    }
}

impl Eq for SPKey {}
impl PartialEq for SPKey {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

#[async_trait]
impl Key for SPKey {
    type Value = usize;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        let behavior = self.data.compute_behavior[self.idx].lock().unwrap().clone();
        match behavior {
            ComputeBehavior::Sleep(duration) => tokio::time::sleep(duration).await,
            ComputeBehavior::WaitFor(semaphore) => {
                drop(semaphore.acquire().await);
            }
            ComputeBehavior::Immediate => {}
        }

        self.data.total_computed.fetch_add(1, Ordering::SeqCst);
        self.idx
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}
