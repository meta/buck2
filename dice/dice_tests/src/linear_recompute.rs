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
use futures::future::FutureExt;
use pagable::Pagable;
use pagable::PagableTypeTag;
use pagable::pagable_typetag;

#[tokio::test]
async fn test_linear_recompute_tracks_deps() {
    #[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    enum K {
        #[display("K::Top")]
        Top,
        #[display("K::Mid({})", _0)]
        Mid(u32),
    }
    impl PagableTypeTag for K {
        fn pagable_type_tag_static() -> &'static str {
            "K"
        }
    }

    #[derive(Allocative, Clone, Copy, Debug, Display, Eq, PartialEq, Hash, Pagable)]
    #[display("Leaf({})", _0)]
    #[pagable_typetag(DiceKeyDyn)]
    struct Leaf(u32);

    impl InjectedKey for Leaf {
        type Value = u32;
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|x, y| x == y)
        }
    }

    #[async_trait]
    impl Key for K {
        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }
        type Value = u32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            match self {
                K::Top => {
                    ctx.with_linear_recompute(|linear| {
                        async move {
                            let mut v = 0;
                            for i in 0..100 {
                                v += linear.get().compute(&K::Mid(i)).await.unwrap();
                            }
                            v
                        }
                        .boxed()
                    })
                    .await
                }
                K::Mid(v) => *ctx.compute(&Leaf(*v)).await.unwrap(),
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

    let ctx = {
        let mut updater = dice.updater(root);
        updater
            .changed_to((0..100).map(|i| (Leaf(i), i)).collect::<Vec<_>>())
            .unwrap();
        updater.commit().await
    };

    assert_eq!(*ctx.compute(&K::Top).await.unwrap(), 4950);

    let ctx = {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Leaf(50), 0)]).unwrap();
        updater.commit().await
    };

    // should be 50 less.
    assert_eq!(*ctx.compute(&K::Top).await.unwrap(), 4900);
}
