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
use std::sync::Barrier;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use assert_matches::assert_matches;
use async_trait::async_trait;
use derivative::Derivative;
use derive_more::Display;
use dice_error::DiceErrorImpl;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use pagable::Pagable;
use pagable::PagablePanic;
use pagable::pagable_typetag;
use tokio::sync::oneshot;

use crate::Dice;
use crate::DiceData;
use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::cycles::DetectCycles;
use crate::api::injected::InjectedKey;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::api::user_data::UserComputationData;
use crate::versions::VersionNumber;

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Foo(i32);

#[async_trait]
impl InjectedKey for Foo {
    type Value = i32;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("GlobalDataKey")]
#[pagable_typetag(DiceKeyDyn)]
struct GlobalDataKey;

#[async_trait]
impl Key for GlobalDataKey {
    type Value = usize;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.global_data()
            .get::<Arc<AtomicUsize>>()
            .expect("test should configure a compute counter")
            .fetch_add(1, Ordering::SeqCst);
        *ctx.compute(&Foo(0)).await.unwrap() as usize
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

fn root_data(value: usize, counter: &Arc<AtomicUsize>) -> DiceData {
    let mut data = DiceData::new();
    data.set(value);
    data.set(counter.dupe());
    data
}

#[tokio::test]
async fn root_data_follows_tracked_inputs() -> anyhow::Result<()> {
    let ordinary_count = Arc::new(AtomicUsize::new(0));
    let dice = Dice::testing_new(root_data(1, &ordinary_count));
    let ordinary = dice.new_root().await;
    let new_ordinary = dice.new_root().await;
    let isolated_count = Arc::new(AtomicUsize::new(0));
    let isolated = dice.new_root_with_data(root_data(2, &isolated_count)).await;
    let other_count = Arc::new(AtomicUsize::new(0));
    let other = dice.new_root_with_data(root_data(3, &other_count)).await;

    for (branch, expected, counter) in [
        (ordinary, 1, &ordinary_count),
        (isolated, 2, &isolated_count),
        (other, 3, &other_count),
    ] {
        let mut updater = dice.updater(branch);
        updater.changed_to(vec![(Foo(0), expected as i32)])?;
        let first = updater.commit().await;
        assert_eq!(*first.compute(&GlobalDataKey).await?, expected);
        drop(first);
        let second = dice.updater(branch).commit().await;
        assert_eq!(*second.compute(&GlobalDataKey).await?, expected);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    let child = dice.fork(dice.head(isolated).await).await;
    dice.delete_branch(isolated);
    let child_ctx = dice.updater(child).commit().await;
    assert_eq!(*child_ctx.compute(&GlobalDataKey).await?, 2);
    assert_eq!(isolated_count.load(Ordering::SeqCst), 1);

    let mut updater = dice.updater(new_ordinary);
    updater.changed_to(vec![(Foo(0), 1)])?;
    let ctx = updater.commit().await;
    assert_eq!(*ctx.compute(&GlobalDataKey).await?, 1);
    assert_eq!(ordinary_count.load(Ordering::SeqCst), 1);

    let shared_count = Arc::new(AtomicUsize::new(0));
    let shared = dice.new_root_with_data(root_data(1, &shared_count)).await;
    let mut updater = dice.updater(shared);
    updater.changed_to(vec![(Foo(0), 1)])?;
    let ctx = updater.commit().await;
    assert_eq!(*ctx.compute(&GlobalDataKey).await?, 1);
    assert_eq!(shared_count.load(Ordering::SeqCst), 0);
    assert!(Arc::ptr_eq(
        ctx.global_data().get::<Arc<AtomicUsize>>()?,
        &shared_count
    ));
    Ok(())
}

#[tokio::test]
async fn retargeting_an_updater_selects_the_new_roots_data() -> anyhow::Result<()> {
    let counter = Arc::new(AtomicUsize::new(0));
    let dice = Dice::testing_new(root_data(1, &counter));
    let ordinary = dice.new_root().await;
    let isolated = dice.new_root_with_data(root_data(2, &counter)).await;
    let mut updater = dice.updater(ordinary);
    updater.retarget(isolated);
    updater.changed_to(vec![(Foo(0), 2)])?;
    updater.commit().await;
    let mut updater = dice.updater(ordinary);
    updater.retarget(isolated);
    let existing = updater.existing_state().await;
    assert_eq!(*existing.global_data().get::<usize>()?, 2);
    assert_eq!(*existing.compute(&GlobalDataKey).await?, 2);
    let committed = updater.commit().await;
    assert_eq!(*committed.compute(&GlobalDataKey).await?, 2);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn cancelled_root_with_data_creation_releases_its_branch_and_data() {
    let dice = Dice::testing_new(DiceData::new());
    for wait_for_reply in [false, true] {
        let counter = Arc::new(AtomicUsize::new(0));
        let creation = dice.new_root_with_data(root_data(1, &counter));
        if wait_for_reply {
            assert!(dice.is_idle().await);
        }
        drop(creation);
        // The second query also follows deletion of an unclaimed reply sent after the first.
        assert!(dice.is_idle().await);
        assert!(dice.is_idle().await);
        assert_eq!(dice.metrics().branch_count, 0);
        assert_eq!(Arc::strong_count(&counter), 1);
    }
}

#[derive(Clone, Dupe, Debug, Derivative, Allocative, Display, PagablePanic)]
#[derivative(PartialEq, Eq, Hash)]
#[display("{:?}", self)]
#[allocative(skip)]
#[pagable_typetag(DiceKeyDyn)]
struct KeyThatRuns {
    #[derivative(Hash = "ignore", PartialEq = "ignore")]
    barrier1: Arc<tokio::sync::Semaphore>,
    #[derivative(Hash = "ignore", PartialEq = "ignore")]
    barrier2: Arc<tokio::sync::Semaphore>,
    #[derivative(Hash = "ignore", PartialEq = "ignore")]
    is_ran: Arc<AtomicBool>,
}

#[async_trait]
impl Key for KeyThatRuns {
    type Value = ();

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        self.barrier1.add_permits(1);
        let _s = self.barrier2.acquire().await.unwrap();
        self.is_ran.store(true, Ordering::SeqCst);
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| true)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[tokio::test]
async fn set_injected_multiple_times_per_commit() -> anyhow::Result<()> {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    {
        let mut ctx = dice.updater(root);
        ctx.changed_to(vec![(Foo(0), 0)])?;
        ctx.changed_to(vec![(Foo(1), 1)])?;

        let ctx = ctx.commit().await;
        assert_eq!(*ctx.compute(&Foo(0)).await?, 0);
        assert_eq!(*ctx.compute(&Foo(1)).await?, 1);
    }

    {
        let mut ctx = dice.updater(root);
        ctx.changed_to(vec![(Foo(0), 0)])?;

        assert_matches!(
            ctx.changed_to(vec![(Foo(0), 1)]),
            Err(err) => assert_matches!(&*err.0, DiceErrorImpl::DuplicateChange(_))
        );
    }

    Ok(())
}

#[tokio::test]
async fn set_injected_with_no_change_no_new_ctx() -> anyhow::Result<()> {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    {
        let mut ctx = dice.updater(root);
        ctx.changed_to(vec![(Foo(0), 0)])?;

        let ctx = ctx.commit().await.0;

        assert_eq!(ctx.get_version(), VersionNumber::testing_new(2));
    }

    {
        let mut ctx = dice.updater(root);
        ctx.changed_to(vec![(Foo(0), 0)])?;

        let ctx = ctx.commit().await.0;
        assert_eq!(ctx.get_version(), VersionNumber::testing_new(2));
    }

    Ok(())
}

#[test]
fn dice_computations_are_parallel() {
    let n_thread = 10;

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(n_thread + 1)
        .build()
        .unwrap();
    let barrier = Arc::new(Barrier::new(n_thread));

    #[derive(Clone, Debug, Display, Derivative, Allocative, PagablePanic)]
    #[derivative(Hash, PartialEq, Eq)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Blocking {
        index: usize,
        #[derivative(PartialEq = "ignore", Hash = "ignore")]
        #[allocative(skip)]
        barrier: Arc<Barrier>,
    }

    #[async_trait]
    impl Key for Blocking {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            self.barrier.wait();
            1
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    rt.block_on(async move {
        let dice = Dice::builder().build(DetectCycles::Disabled);
        let root = dice.new_root().await;
        let mut sum = 0;

        let dice = &dice;
        let barrier = &barrier;

        let futs = (0..n_thread)
            .map(|i| async move {
                *dice
                    .updater(root)
                    .commit()
                    .await
                    .compute(&Blocking {
                        index: i,
                        barrier: barrier.dupe(),
                    })
                    .await
                    .unwrap()
            })
            .collect::<Vec<_>>();

        dice_futures::join::join_all(futs)
            .await
            .iter()
            .for_each(|res| sum += *res);

        assert_eq!(sum, n_thread);
    })
}

#[tokio::test]
async fn different_data_per_compute_ctx() {
    struct U(usize);

    #[derive(Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct DataRequest(u8);

    #[async_trait]
    impl Key for DataRequest {
        type Value = usize;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            ctx.per_transaction_data().data.get::<U>().unwrap().0
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
    let per_cmd_data0 = {
        let mut d = UserComputationData::new();
        d.data.set(U(0));
        d
    };
    let per_cmd_data1 = {
        let mut d = UserComputationData::new();
        d.data.set(U(1));
        d
    };

    let ctx0 = dice.updater_with_data(root, per_cmd_data0).commit().await;

    let ctx1 = dice.updater_with_data(root, per_cmd_data1).commit().await;

    let request0 = ctx0.compute(&DataRequest(0));
    let request1 = ctx1.compute(&DataRequest(1));

    assert_eq!(*request0.await.unwrap(), 0);
    assert_eq!(*request1.await.unwrap(), 1);
}

#[tokio::test]
async fn invalid_update() {
    #[derive(Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Allocative, Pagable)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Invalid;

    #[async_trait]
    impl Key for Invalid {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            unimplemented!("not needed for test")
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| unimplemented!("not needed for test"))
        }

        fn validity(_x: &Self::Value) -> bool {
            false
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let dice = Dice::testing_new(DiceData::new());
    let root = dice.new_root().await;
    let mut updater = dice.updater(root);

    assert!(updater.changed_to([(Invalid, ())]).is_err());
}

#[derive(
    Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Fib(u8);

#[async_trait]
impl Key for Fib {
    type Value = Result<u64, Arc<anyhow::Error>>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        if self.0 > 93 {
            return Err(Arc::new(anyhow::anyhow!("that's too big")));
        }
        if self.0 < 2 {
            return Ok(self.0 as u64);
        }
        let (a, b) = {
            ctx.compute2(
                async |ctx| ctx.compute(&Fib(self.0 - 2)).await,
                async |ctx| ctx.compute(&Fib(self.0 - 1)).await,
            )
            .await
        };
        match (a, b) {
            (Ok(a), Ok(b)) => Ok(a.dupe()? + b.dupe()?),
            _ => Err(Arc::new(anyhow::anyhow!("some dice error"))),
        }
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| match (x, y) {
            (Ok(x), Ok(y)) => x == y,
            _ => false,
        })
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// Test that dropping request cancel execution. Test scenario is:
/// The test has 1 barrier and 1 semaphore:
/// - barrier is used by the task to signal that the task is started, and that the task doesn't progress
///   to complete.
/// - semaphore ensures that the task doesn't run until the main thread drops the request
/// 1. we spawn a task
/// 2. assert task is spawned
/// 3. drop the request future
/// 4. ensure the task is cancelled since the task will panic if it proceeds beyond the semaphore
#[tokio::test]
async fn dropping_request_future_cancels_execution() {
    #[derive(Debug)]
    struct DropSignal(Option<oneshot::Sender<()>>);

    impl Drop for DropSignal {
        fn drop(&mut self) {
            self.0.take().unwrap().send(()).unwrap();
        }
    }

    #[derive(Clone, Dupe, Debug, Derivative, Allocative, Display, PagablePanic)]
    #[derivative(PartialEq, Eq, Hash)]
    #[display("{:?}", self)]
    #[allocative(skip)]
    #[pagable_typetag(DiceKeyDyn)]
    struct KeyThatShouldntRun {
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        barrier1: Arc<tokio::sync::Barrier>,
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        barrier2: Arc<tokio::sync::Semaphore>,
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        drop_signal: Arc<Mutex<Option<DropSignal>>>,
    }

    #[async_trait]
    impl Key for KeyThatShouldntRun {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            let _drop_signal = self.drop_signal.lock().unwrap().take();

            self.barrier1.wait().await;
            let _guard = self.barrier2.acquire().await.unwrap();

            panic!("shouldn't run")
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let barrier1 = Arc::new(tokio::sync::Barrier::new(2));
    let barrier2 = Arc::new(tokio::sync::Semaphore::new(0));

    let (tx, rx) = oneshot::channel();
    let drop_signal = DropSignal(Some(tx));

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let ctx = dice.updater(root).commit().await;

    let key = KeyThatShouldntRun {
        barrier1: barrier1.dupe(),
        barrier2: barrier2.dupe(),
        drop_signal: Arc::new(Mutex::new(Some(drop_signal))),
    };
    let req = ctx.compute(&key);

    // ensure that the key starts computing
    barrier1.wait().await;

    drop(req);

    barrier2.add_permits(1);

    rx.await.unwrap();
}

/// Test that dropping request cancel execution. Test scenario is:
/// The test has 2 barriers:
/// - first is used by the task to signal that the task is started, and that the task doesn't progress
///   to complete.
/// - second ensures that the task doesn't run until the main thread drops the one of the request
/// 1. we spawn a task, but have two requests attached to it
/// 2. assert task is spawned
/// 3. drop one of the request future
/// 4. ensure the task is still finishes and runs by checking the `is-ran` atomic
#[tokio::test]
async fn dropping_request_future_doesnt_cancel_if_multiple_requests_active() {
    let barrier1 = Arc::new(tokio::sync::Semaphore::new(0));
    let barrier2 = Arc::new(tokio::sync::Semaphore::new(0));
    let is_ran = Arc::new(AtomicBool::new(false));

    let key = &KeyThatRuns {
        barrier1: barrier1.dupe(),
        barrier2: barrier2.dupe(),
        is_ran: is_ran.dupe(),
    };

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let ctx = dice.updater(root).commit().await.0;
    let mut ctx = ctx.as_computations();
    let (req1, req2) = ctx.compute2(
        |ctx| ctx.compute(key).boxed(),
        |ctx| ctx.compute(key).boxed(),
    );

    // ensure that the key starts computing
    let _b = barrier1.acquire().await;

    drop(req1);

    barrier2.add_permits(1);

    // req2 still succeed
    req2.await.unwrap();

    assert!(is_ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_dice_usable_after_cancellations() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let ctx = dice.updater(root).commit().await;

    let barrier1 = Arc::new(tokio::sync::Semaphore::new(0));
    let barrier2 = Arc::new(tokio::sync::Semaphore::new(0));
    let is_ran = Arc::new(AtomicBool::new(false));

    let key = KeyThatRuns {
        barrier1: barrier1.dupe(),
        barrier2: barrier2.dupe(),
        is_ran: is_ran.dupe(),
    };

    let req1 = ctx.compute(&key);

    // ensure that the key starts computing
    let _b = barrier1.acquire().await;

    // cancel and await for cancellation
    drop(req1);
    drop(ctx);

    dice.wait_for_idle().await;

    assert!(!is_ran.load(Ordering::Acquire));

    let ctx = dice.updater(root).commit().await;

    // req2 still succeed. Note that due to dice caching, even if we make a new key, the same
    // instance would be used, so just use the same one.
    let req2 = ctx.compute(&key);

    barrier2.add_permits(1);

    req2.await.unwrap();

    assert!(is_ran.load(Ordering::SeqCst));
}

#[tokio::test]
async fn test_is_idle_respects_active_transactions() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let ctx = dice.updater(root).commit().await;

    let barrier1 = Arc::new(tokio::sync::Semaphore::new(0));
    let barrier2 = Arc::new(tokio::sync::Semaphore::new(0));
    let is_ran = Arc::new(AtomicBool::new(false));

    let key = KeyThatRuns {
        barrier1: barrier1.dupe(),
        barrier2: barrier2.dupe(),
        is_ran: is_ran.dupe(),
    };

    let req1 = ctx.compute(&key);
    assert!(!dice.is_idle().await);

    barrier2.add_permits(1);
    req1.await.unwrap();

    // Alive and computing nothing is still not idle.
    assert!(!dice.is_idle().await);
    drop(ctx);
    assert!(dice.is_idle().await);
    dice.wait_for_idle().await;
}

#[tokio::test]
async fn compute_with_key_returns_the_shared_key_allocation() -> anyhow::Result<()> {
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Leaf(usize);

    #[async_trait]
    impl Key for Leaf {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            self.0 + 1
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Requester(usize);

    #[async_trait]
    impl Key for Requester {
        type Value = (Arc<Leaf>, usize);

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            let (key, value) = ctx.compute_with_key(&Leaf(7)).await.unwrap();
            (key, *value)
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
    let ctx = dice.updater(root).commit().await;
    let mut ctx = ctx.ctx();

    let (a, b) = ctx
        .compute2(
            async |ctx| ctx.compute(&Requester(1)).await,
            async |ctx| ctx.compute(&Requester(2)).await,
        )
        .await;
    let (a, b) = (a?, b?);

    assert_eq!(a.1, 8);
    assert_eq!(b.1, 8);
    // Each requester built its own `Leaf(7)`; both got the one DICE kept.
    assert!(Arc::ptr_eq(&a.0, &b.0));
    let (direct, _) = ctx.compute_with_key(&Leaf(7)).await?;
    assert!(Arc::ptr_eq(&a.0, &direct));

    Ok(())
}

/// A branch starts out seeing its fork point, shares the values computed there, and diverges
/// from the branch it was forked from with its own commits.
#[tokio::test]
async fn a_branch_diverges_from_its_fork_point() -> anyhow::Result<()> {
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Derived;

    static DERIVED_COMPUTES: AtomicUsize = AtomicUsize::new(0);

    #[async_trait]
    impl Key for Derived {
        type Value = i32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            DERIVED_COMPUTES.fetch_add(1, Ordering::SeqCst);
            *ctx.compute(&Foo(0)).await.unwrap() + 10
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

    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Foo(0), 1)])?;
    let root_ctx = updater.commit().await;
    assert_eq!(*root_ctx.compute(&Derived).await?, 11);

    let branch = dice.fork(root_ctx.0.get_version()).await;
    assert_ne!(branch, root);
    let mut updater = dice.updater(branch);
    assert_eq!(updater.branch(), branch);

    // The branch sees its fork point, and reuses what was computed there.
    let fork_point = updater.existing_state().await;
    assert_eq!(fork_point.0.get_version().branch(), branch);
    assert_eq!(*fork_point.compute(&Foo(0)).await?, 1);
    assert_eq!(*fork_point.compute(&Derived).await?, 11);
    assert_eq!(DERIVED_COMPUTES.load(Ordering::SeqCst), 1);

    // A commit on the branch changes only the branch.
    updater.changed_to(vec![(Foo(0), 2)])?;
    let branch_ctx = updater.commit().await;
    assert_eq!(branch_ctx.0.get_version().branch(), branch);
    assert_eq!(dice.head(branch).await, branch_ctx.0.get_version());
    assert_eq!(*branch_ctx.compute(&Derived).await?, 12);
    assert_eq!(DERIVED_COMPUTES.load(Ordering::SeqCst), 2);

    assert_eq!(*root_ctx.compute(&Derived).await?, 11);
    let root_now = dice.updater(root).existing_state().await;
    assert_eq!(root_now.0.get_version(), root_ctx.0.get_version());
    assert_eq!(*root_now.compute(&Derived).await?, 11);
    assert_eq!(DERIVED_COMPUTES.load(Ordering::SeqCst), 2);

    Ok(())
}

/// `wait_for_idle` returns once the last transaction is dropped, not just once computations end.
#[tokio::test]
async fn waiting_for_idle_waits_for_transactions_to_be_dropped() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let ctx = dice.updater(root).commit().await;

    let waited = tokio::spawn(dice.wait_for_idle());
    tokio::task::yield_now().await;
    assert!(!waited.is_finished());

    drop(ctx);
    waited.await.unwrap();
    assert!(dice.is_idle().await);
}

/// A new root starts with nothing injected. It shares with the branches before it only what a
/// certificate lets it: a value whose inputs it has injected identically is reused, one whose
/// inputs differ is computed anew, and deleting the root leaves the others alone.
#[tokio::test]
async fn a_new_root_shares_only_what_certificates_allow() -> anyhow::Result<()> {
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Counted;

    static COUNTED_COMPUTES: AtomicUsize = AtomicUsize::new(0);

    #[async_trait]
    impl Key for Counted {
        type Value = i32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            COUNTED_COMPUTES.fetch_add(1, Ordering::SeqCst);
            *ctx.compute(&Foo(0)).await.unwrap() + 10
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
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Foo(0), 1)])?;
    let first_ctx = updater.commit().await;
    assert_eq!(*first_ctx.compute(&Counted).await?, 11);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 1);

    let other = dice.new_root().await;
    assert_ne!(other, root);

    // The same input injected at the new root makes the first branch's certificate hold there.
    let mut updater = dice.updater(other);
    updater.changed_to(vec![(Foo(0), 1)])?;
    let root_ctx = updater.commit().await;
    assert_eq!(root_ctx.version().branch(), other);
    assert_eq!(*root_ctx.compute(&Counted).await?, 11);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 1);

    // A different input does not.
    let mut updater = dice.updater(other);
    updater.changed_to(vec![(Foo(0), 2)])?;
    let root_ctx = updater.commit().await;
    assert_eq!(*root_ctx.compute(&Counted).await?, 12);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 2);
    assert_eq!(dice.pagable_node_counts().await.resident, 2);

    drop(root_ctx);
    dice.delete_branch(other);
    assert_eq!(dice.pagable_node_counts().await.resident, 1);
    assert_eq!(*first_ctx.compute(&Counted).await?, 11);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 2);
    Ok(())
}

/// Changes recorded before an updater is retargeted commit to the new branch, and its existing
/// state is that branch's.
#[tokio::test]
async fn a_retargeted_updater_commits_to_the_new_branch() -> anyhow::Result<()> {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let first = dice.new_root().await;
    let mut updater = dice.updater(first);
    updater.changed_to(vec![(Foo(0), 1)])?;

    let root = dice.new_root().await;
    updater.retarget(root);
    assert_eq!(updater.branch(), root);
    assert_eq!(updater.existing_state().await.version().branch(), root);

    let ctx = updater.commit().await;
    assert_eq!(ctx.version().branch(), root);
    assert_eq!(*ctx.compute(&Foo(0)).await?, 1);
    assert_eq!(dice.head(first).await, VersionNumber::testing_new(1));
    Ok(())
}

/// Deleting a branch releases what only it retained. A transaction still running on it is still
/// served, from its own cache and by computing afresh, but nothing it computes is kept.
#[tokio::test]
async fn deleting_a_branch_releases_its_values_and_strands_its_transactions() -> anyhow::Result<()>
{
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Counted;

    static COUNTED_COMPUTES: AtomicUsize = AtomicUsize::new(0);

    #[async_trait]
    impl Key for Counted {
        type Value = i32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            COUNTED_COMPUTES.fetch_add(1, Ordering::SeqCst);
            *ctx.compute(&Foo(0)).await.unwrap() + 10
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
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Foo(0), 1)])?;
    let root_ctx = updater.commit().await;

    let branch = dice.fork(root_ctx.version()).await;
    let mut updater = dice.updater(branch);
    updater.changed_to(vec![(Foo(0), 2)])?;
    let branch_ctx = updater.commit().await;
    assert_eq!(*branch_ctx.compute(&Counted).await?, 12);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 1);
    assert_eq!(dice.pagable_node_counts().await.resident, 1);

    dice.delete_branch(branch);
    assert_eq!(dice.pagable_node_counts().await.resident, 0);

    // The stranded transaction still answers from its own cache, and computes what it has not
    // seen; neither is kept.
    assert_eq!(*branch_ctx.compute(&Counted).await?, 12);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 1);
    assert_eq!(*branch_ctx.compute(&Foo(0)).await?, 2);
    let stranded = dice.updater(branch);
    drop(stranded);
    assert_eq!(dice.pagable_node_counts().await.resident, 0);

    // The branch it was forked from never saw any of it.
    assert_eq!(*root_ctx.compute(&Counted).await?, 11);
    assert_eq!(COUNTED_COMPUTES.load(Ordering::SeqCst), 2);
    assert_eq!(dice.pagable_node_counts().await.resident, 1);
    Ok(())
}

/// Deletion takes the branch's injected keys with everything else. A transaction stranded on it
/// is still served the ones it has seen, from its own cache, and one it requests for the first
/// time fails as if it had never been injected.
// `InjectedKey::compute` panics in a spawned worker. With `panic = "abort"`, that panic ends the
// test process. With `panic = "unwind"` (as with `cargo test`), the worker poisons the task, and
// the requester panics because the task is poisoned.
#[tokio::test]
#[cfg_attr(
    panic = "abort",
    should_panic(
        expected = "Injected Keys must be injected onto the graph before being requested"
    )
)]
#[cfg_attr(
    panic = "unwind",
    should_panic(expected = "the key's computation panicked")
)]
async fn a_stranded_transaction_fails_on_an_unseen_injected_key() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let root_ctx = dice.updater(root).commit().await;
    let branch = dice.fork(root_ctx.version()).await;
    let mut updater = dice.updater(branch);
    updater.changed_to(vec![(Foo(0), 1), (Foo(1), 2)]).unwrap();
    let branch_ctx = updater.commit().await;
    assert_eq!(*branch_ctx.compute(&Foo(0)).await.unwrap(), 1);

    dice.delete_branch(branch);
    assert_eq!(*branch_ctx.compute(&Foo(0)).await.unwrap(), 1);
    branch_ctx.compute(&Foo(1)).await.unwrap();
}

/// A `fork` or `new_root` whose caller stops waiting for the answer does not leak the branch,
/// whether the caller gave up before the actor made it or after.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn an_unawaited_branch_is_deleted() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let version = dice.updater(root).commit().await.version();
    assert_eq!(dice.metrics().branch_count, 1);

    // Dropped before the actor got to it, so that its answer cannot be sent. The round trip
    // after it returns once the actor has moved on.
    drop(dice.fork(version));
    dice.head(root).await;
    assert_eq!(dice.metrics().branch_count, 1);

    // Dropped after the actor answered, with the answer unread.
    let unclaimed = dice.new_root();
    dice.head(root).await;
    drop(unclaimed);
    assert_eq!(dice.metrics().branch_count, 1);

    assert_ne!(dice.new_root().await, root);
    assert_eq!(dice.metrics().branch_count, 2);
}

/// Idleness can be asked of one branch: work on another leaves it idle.
#[tokio::test]
async fn idleness_is_per_branch() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let fork_point = dice.updater(root).commit().await.version();
    let branch = dice.fork(fork_point).await;
    let branch_ctx = dice.updater(branch).commit().await;

    let barrier1 = Arc::new(tokio::sync::Semaphore::new(0));
    let barrier2 = Arc::new(tokio::sync::Semaphore::new(0));
    let key = KeyThatRuns {
        barrier1: barrier1.dupe(),
        barrier2: barrier2.dupe(),
        is_ran: Arc::new(AtomicBool::new(false)),
    };

    let req = branch_ctx.compute(&key);
    barrier1.acquire().await.unwrap().forget();

    assert!(!dice.is_idle().await);
    assert!(!dice.is_idle_on(branch).await);
    assert!(dice.is_idle_on(root).await);
    dice.wait_for_idle_on(root).await;

    // The same holds once the transaction is gone and the compute is only draining.
    drop(req);
    drop(branch_ctx);
    assert!(dice.is_idle_on(root).await);

    barrier2.add_permits(1);
    dice.wait_for_idle_on(branch).await;
    assert!(dice.is_idle_on(branch).await);
    assert!(dice.is_idle().await);
}

/// A compute that is in a critical section when its transaction is dropped keeps running, the
/// keys it requests from there are computed for it, and what it computes is kept.
#[tokio::test]
async fn a_dropped_transaction_still_serves_its_computes() {
    #[derive(Clone, Dupe, Debug, Derivative, Allocative, Display, PagablePanic)]
    #[derivative(PartialEq, Eq, Hash)]
    #[display("{:?}", self)]
    #[allocative(skip)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Outer {
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        started: Arc<tokio::sync::Semaphore>,
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        proceed: Arc<tokio::sync::Semaphore>,
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        done: Arc<tokio::sync::Semaphore>,
        #[derivative(Hash = "ignore", PartialEq = "ignore")]
        inner_value: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Key for Outer {
        type Value = ();

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            cancellations: &CancellationContext,
        ) -> Self::Value {
            cancellations
                .critical_section(|| async move {
                    self.started.add_permits(1);
                    let _p = self.proceed.acquire().await.unwrap();
                    let inner = *ctx.compute(&Inner).await.unwrap();
                    self.inner_value.store(inner, Ordering::SeqCst);
                    self.done.add_permits(1);
                })
                .await
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Inner;

    static INNER_COMPUTES: AtomicUsize = AtomicUsize::new(0);

    #[async_trait]
    impl Key for Inner {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            INNER_COMPUTES.fetch_add(1, Ordering::SeqCst);
            42
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
    let ctx = dice.updater(root).commit().await;

    let key = Outer {
        started: Arc::new(tokio::sync::Semaphore::new(0)),
        proceed: Arc::new(tokio::sync::Semaphore::new(0)),
        done: Arc::new(tokio::sync::Semaphore::new(0)),
        inner_value: Arc::new(AtomicUsize::new(0)),
    };

    let req = ctx.compute(&key);
    key.started.acquire().await.unwrap().forget();

    // Everything that refers to the compute goes away while it sits in its critical section.
    drop(req);
    drop(ctx);
    assert!(!dice.is_idle().await);

    key.proceed.add_permits(1);
    key.done.acquire().await.unwrap().forget();
    assert_eq!(key.inner_value.load(Ordering::SeqCst), 42);

    dice.wait_for_idle().await;
    assert!(dice.is_idle().await);

    // The value the abandoned compute produced for `Inner` was kept.
    let ctx = dice.updater(root).commit().await;
    assert_eq!(*ctx.compute(&Inner).await.unwrap(), 42);
    assert_eq!(INNER_COMPUTES.load(Ordering::SeqCst), 1);
}
