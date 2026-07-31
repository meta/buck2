/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::panic::AssertUnwindSafe;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use pagable::Pagable;
use pagable::pagable_typetag;

// dice graph storage needs to not reuse deps just because the value hasn't changed
#[tokio::test]
async fn test_dice_recompute_doesnt_reuse_wrong_deps() -> anyhow::Result<()> {
    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Leaf(u32);

    impl InjectedKey for Leaf {
        type Value = u32;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| *x == *y)
        }
    }

    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Derived;

    #[async_trait]
    impl Key for Derived {
        type Value = u32;

        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> u32 {
            let x = *ctx.compute(&Leaf(0)).await.unwrap();
            *ctx.compute(&Leaf(x)).await.unwrap()
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| *x == *y)
        }
    }

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;

    let mut updater = dice.updater(root);
    updater.changed_to([(Leaf(0), 1), (Leaf(1), 100), (Leaf(2), 200)])?;
    let ctx1 = updater.commit().await;

    let mut updater = dice.updater(root);
    updater.changed_to([(Leaf(0), 1), (Leaf(1), 300), (Leaf(2), 200)])?;
    let ctx2 = updater.commit().await;

    let mut updater = dice.updater(root);
    updater.changed_to([(Leaf(0), 2), (Leaf(1), 400), (Leaf(2), 100)])?;
    let ctx3 = updater.commit().await;

    assert_eq!(*ctx1.compute(&Derived).await.unwrap(), 100);
    assert_eq!(*ctx3.compute(&Derived).await.unwrap(), 100);
    assert_eq!(*ctx2.compute(&Derived).await.unwrap(), 300);

    Ok(())
}

#[tokio::test]
async fn deleting_the_branch_does_not_break_an_ongoing_computation() -> anyhow::Result<()> {
    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Fib(u32);

    #[async_trait]
    impl Key for Fib {
        type Value = Option<u32>;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Option<u32> {
            Some(match self.0 {
                0 => 1,
                1 => 1,
                n => {
                    (*ctx.compute(&Fib(n - 1)).await.ok()?)?
                        + (*ctx.compute(&Fib(n - 2)).await.ok()?)?
                }
            })
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| false)
        }
    }

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let updater = dice.updater(root);
    let ctx1 = updater.commit().await;

    ctx1.compute(&Fib(3)).await?;

    dice.delete_branch(root);

    assert_eq!(*ctx1.compute(&Fib(10)).await?, Some(89));

    Ok(())
}

/// Regression: a dep dropped from a key's compute on a later run must not
/// leave a stale reverse-dep edge behind. Once `K` stops depending on
/// `Right`, later changes to `Right` must not invalidate `K` or drag it
/// through a dep-check on the next lookup.
#[tokio::test]
async fn dropped_dep_does_not_leave_stale_rdep_edge() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::sync::Mutex;

    use allocative::Allocative;
    use dice::DiceEvent;
    use dice::DiceEventListener;
    use dice::UserComputationData;
    use dupe::Dupe;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("Selector")]
    #[pagable_typetag(DiceKeyDyn)]
    struct Selector;
    impl InjectedKey for Selector {
        type Value = bool;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        fn equality_behavior() -> dice::EqualityBehavior<Self::Value> {
            dice::EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("Left")]
    #[pagable_typetag(DiceKeyDyn)]
    struct Left;
    impl InjectedKey for Left {
        type Value = u32;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        fn equality_behavior() -> dice::EqualityBehavior<Self::Value> {
            dice::EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("Right")]
    #[pagable_typetag(DiceKeyDyn)]
    struct Right;
    impl InjectedKey for Right {
        type Value = u32;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        fn equality_behavior() -> dice::EqualityBehavior<Self::Value> {
            dice::EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    /// `K = Left + Right` when `Selector` is true, else `K = Left`. The
    /// dep set includes `Right` only in the first branch, so flipping
    /// `Selector` shrinks `K`'s deps.
    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("K")]
    #[pagable_typetag(DiceKeyDyn)]
    struct K;

    #[async_trait]
    impl Key for K {
        type Value = u32;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> u32 {
            let sel = *ctx.compute(&Selector).await.unwrap();
            let l = *ctx.compute(&Left).await.unwrap();
            if sel {
                l + *ctx.compute(&Right).await.unwrap()
            } else {
                l
            }
        }
        fn equality_behavior() -> dice::EqualityBehavior<Self::Value> {
            dice::EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    /// Records the sequence of `DiceEvent`s so the test can assert what
    /// dice did (or did not do) for a given key on a given lookup.
    #[derive(Allocative)]
    struct Recorder {
        #[allocative(skip)]
        events: Arc<Mutex<Vec<DiceEvent>>>,
    }
    impl DiceEventListener for Recorder {
        fn event(&self, ev: DiceEvent) {
            self.events.lock().unwrap().push(ev);
        }
    }

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    // v1: Selector=true, Left=1, Right=10. K depends on {Selector, Left, Right}.
    let mut updater = dice.updater(root);
    updater.changed_to([(Selector, true)])?;
    updater.changed_to([(Left, 1u32)])?;
    updater.changed_to([(Right, 10u32)])?;
    let ctx = updater.commit().await;
    assert_eq!(*ctx.compute(&K).await.unwrap(), 11);
    drop(ctx);

    // v2: flip Selector to false, forcing K's recompute. K now depends on
    // {Selector, Left} only — Right has been dropped from the dep set.
    let mut updater = dice.updater(root);
    updater.changed_to([(Selector, false)])?;
    let ctx = updater.commit().await;
    assert_eq!(*ctx.compute(&K).await.unwrap(), 1);
    drop(ctx);

    // v3: change Right. With the fix, K is not in Right's rdep set, so
    // Right's change must not dirty K. On lookup, K's slot is Match — no
    // ComputeStarted, no CheckDepsStarted for K.
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut data = UserComputationData::new();
    data.tracker = Arc::new(Recorder {
        events: events.clone(),
    });
    let mut updater = dice.updater_with_data(root, data);
    updater.changed_to([(Right, 999u32)])?;
    let ctx = updater.commit().await;
    assert_eq!(*ctx.compute(&K).await.unwrap(), 1);

    let saw_k_touched = events.lock().unwrap().iter().any(|ev| {
        matches!(
            ev,
            DiceEvent::ComputeStarted { key_type: "K" }
                | DiceEvent::CheckDepsStarted { key_type: "K" }
        )
    });
    assert!(
        !saw_k_touched,
        "K should not be recomputed nor dep-checked at v3 (Right is no longer its dep)",
    );

    Ok(())
}

/// A key whose computation panics must make its waiters panic. A cancelled task without a value
/// would make its waiters wait forever.
#[tokio::test]
async fn test_panicking_key_panics_its_waiters() {
    #[derive(
        Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
    )]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct NeverInjected;

    impl InjectedKey for NeverInjected {
        type Value = u32;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let ctx = dice.updater(root).commit().await;

    // `InjectedKey::compute` panics when the key was not injected.
    let result = AssertUnwindSafe(async { ctx.compute(&NeverInjected).await })
        .catch_unwind()
        .await;
    assert!(result.is_err(), "requesting a panicking key must panic");

    // A second request must read the poisoned task instead of waiting on it.
    let again = AssertUnwindSafe(async { ctx.compute(&NeverInjected).await })
        .catch_unwind()
        .await;
    assert!(again.is_err(), "the poisoned task must stay poisoned");
}
