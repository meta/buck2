/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::sync::Arc;
use std::sync::Mutex;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::ActivationData;
use dice::ActivationTracker;
use dice::BranchId;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::DynKey;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice::NoValueSerialize;
use dice::UserComputationData;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::PagableTypeTag;
use pagable::pagable_typetag;

#[tokio::test]
async fn test_a_multiversion_bug() {
    #[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Leaf;

    #[async_trait]
    impl InjectedKey for Leaf {
        type Value = u32;

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    #[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    enum Derived {
        #[display("Derived::Top")]
        Top,
        #[display("Derived::Mid")]
        Mid,
    }
    impl PagableTypeTag for Derived {
        fn pagable_type_tag_static() -> &'static str {
            "Derived"
        }
    }

    #[async_trait]
    impl Key for Derived {
        type Value = u32;

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            match self {
                Derived::Top => *ctx.compute(&Derived::Mid).await.unwrap(),
                Derived::Mid => *ctx.compute(&Leaf).await.unwrap(),
            }
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    let dice = {
        let builder = Dice::builder();
        builder.build(DetectCycles::Enabled)
    };
    let root = dice.new_root().await;

    let ctx1 = {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Leaf, 1)]).unwrap();
        updater.commit().await
    };

    let ctx2 = {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Leaf, 2)]).unwrap();
        updater.commit().await
    };

    let ctx3 = {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Leaf, 1)]).unwrap();
        updater.commit().await
    };

    assert_eq!(*ctx1.compute(&Derived::Mid).await.unwrap(), 1);
    assert_eq!(*ctx3.compute(&Derived::Mid).await.unwrap(), 1);

    assert_eq!(*ctx2.compute(&Derived::Mid).await.unwrap(), 2);
    assert_eq!(*ctx2.compute(&Derived::Top).await.unwrap(), 2);

    assert_eq!(*ctx1.compute(&Derived::Top).await.unwrap(), 1);
    assert_eq!(*ctx3.compute(&Derived::Top).await.unwrap(), 1);
}

/// Records, per key, whether each activation reused the key's value or evaluated it.
#[derive(Default, Allocative)]
struct ActivationRecorder {
    #[allocative(skip)]
    events: Mutex<Vec<(String, bool)>>,
}

impl ActivationTracker for ActivationRecorder {
    fn key_activated(
        &self,
        key: &DynKey,
        _deps: &mut dyn Iterator<Item = &DynKey>,
        activation_data: ActivationData,
    ) {
        let reused = matches!(activation_data, ActivationData::Reused);
        self.events.lock().unwrap().push((key.to_string(), reused));
    }
}

impl ActivationRecorder {
    fn reused_flags_for(&self, key: &str) -> Vec<bool> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, reused)| *reused)
            .collect()
    }
}

/// An injected key returned to an earlier value gets that value's earlier revision back,
/// so a dependent last computed against it revalidates instead of recomputing. Moving the
/// injected key to a third value, by contrast, recomputes the dependent.
#[tokio::test]
async fn injected_key_returning_to_earlier_value_revalidates_dependents() {
    #[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Leaf;

    #[async_trait]
    impl InjectedKey for Leaf {
        type Value = u32;

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    #[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    #[display("Dep")]
    #[pagable_typetag(DiceKeyDyn)]
    struct Dep;

    #[async_trait]
    impl Key for Dep {
        type Value = u32;

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            *ctx.compute(&Leaf).await.unwrap()
        }

        // With value equality, a recompute of `Dep` would also re-find its revision and be
        // indistinguishable here from a revalidation; without it, only revalidation reports
        // the key as reused.
        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::AlwaysUnequal
        }
    }

    async fn compute_dep_with_leaf(dice: &Arc<Dice>, root: BranchId, leaf: u32) -> Vec<bool> {
        let recorder = Arc::new(ActivationRecorder::default());
        let mut updater = dice.updater_with_data(
            root,
            UserComputationData {
                activation_tracker: Some(recorder.dupe() as Arc<dyn ActivationTracker>),
                ..Default::default()
            },
        );
        updater.changed_to(vec![(Leaf, leaf)]).unwrap();
        let ctx = updater.commit().await;
        assert_eq!(*ctx.compute(&Dep).await.unwrap(), leaf);
        drop(ctx);
        recorder.reused_flags_for("Dep")
    }

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    assert_eq!(compute_dep_with_leaf(&dice, root, 1).await, vec![false]);
    // `Dep` is not computed at this version: a recompute here would record the edge to
    // the second value, and returning to the first would then be a genuine change for it.
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Leaf, 2)]).unwrap();
    drop(updater.commit().await);
    assert_eq!(compute_dep_with_leaf(&dice, root, 1).await, vec![true]);

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    assert_eq!(compute_dep_with_leaf(&dice, root, 1).await, vec![false]);
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Leaf, 2)]).unwrap();
    drop(updater.commit().await);
    assert_eq!(compute_dep_with_leaf(&dice, root, 3).await, vec![false]);
}
