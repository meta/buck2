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
use std::hash::Hasher;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::DiceProjectionComputations;
use dice::DiceProjectionDyn;
use dice::EqualityBehavior;
use dice::Key;
use dice::ProjectionKey;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct BaseK;

#[async_trait]
impl Key for BaseK {
    type Value = ();

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| true)
    }
}

#[tokio::test]
async fn concurrent_identical_requests_are_reused() -> anyhow::Result<()> {
    #[derive(Allocative, Clone, Debug, Display, Pagable)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceProjectionDyn)]
    struct ComputeOnce(
        #[allocative(skip)]
        #[pagable(discard = "(|| unimplemented!())()")]
        Arc<AtomicU8>,
    );

    impl PartialEq for ComputeOnce {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl Eq for ComputeOnce {}

    impl Hash for ComputeOnce {
        fn hash<H: Hasher>(&self, _state: &mut H) {}
    }

    impl ProjectionKey for ComputeOnce {
        type DeriveFromKey = BaseK;
        type Value = ();

        fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
            dice::NoValueSerialize::<Self::Value>::new()
        }

        fn compute(
            &self,
            _derive_from: &<<Self as ProjectionKey>::DeriveFromKey as Key>::Value,
            _ctx: &DiceProjectionComputations,
        ) -> Self::Value {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| true)
        }
    }

    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;

    let count = Arc::new(AtomicU8::new(0));

    let ctx = dice.updater(root).commit().await;

    let k = ComputeOnce(count.dupe());

    let base = ctx.ctx().compute_opaque(&BaseK).await?;

    ctx.ctx().projection(&base, &k)?;

    assert_eq!(count.load(Ordering::SeqCst), 1);
    ctx.ctx().projection(&base, &k)?;
    assert_eq!(count.load(Ordering::SeqCst), 1);

    // call base again but technically same key
    let base = ctx.ctx().compute_opaque(&BaseK).await?;

    ctx.ctx().projection(&base, &k)?;
    assert_eq!(count.load(Ordering::SeqCst), 1);

    Ok(())
}
