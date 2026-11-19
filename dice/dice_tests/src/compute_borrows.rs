/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Tests that `&'d Value` references returned from `compute` outlive the `&mut` borrows of
//! the ctx that produced them, i.e. that they can be held across subsequent uses of the ctx.

use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::EqualityBehavior;
use dice::Key;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(
    Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Leaf(u32);

#[async_trait]
impl Key for Leaf {
    type Value = Arc<u32>;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        Arc::new(self.0)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

#[derive(
    Clone, Copy, Dupe, Display, Debug, Eq, PartialEq, Hash, Allocative, Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Top;

#[async_trait]
impl Key for Top {
    type Value = u32;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        // References obtained inside parallel branches must be holdable after the join completes.
        let refs: Vec<&Arc<u32>> = ctx
            .compute_join(1..4u32, async |ctx, i| ctx.compute(&Leaf(i)).await.unwrap())
            .await;

        // ...including refs from *nested* parallel branches escaping both levels.
        let nested: Vec<&Arc<u32>> = ctx
            .compute_join(1..3u32, async |ctx, i| {
                ctx.compute_join(1..3u32, async move |ctx, j| {
                    ctx.compute(&Leaf(100 * i + j)).await.unwrap()
                })
                .await
            })
            .await
            .into_iter()
            .flatten()
            .collect();

        // ...and across a subsequent sequential compute on the same ctx.
        let last: &Arc<u32> = ctx.compute(&Leaf(1000)).await.unwrap();

        refs.into_iter().chain(nested).map(|v| **v).sum::<u32>() + **last
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

#[tokio::test]
async fn test_compute_results_outlive_ctx_borrows() -> anyhow::Result<()> {
    let dice = Dice::builder().build(DetectCycles::Enabled);
    let root = dice.new_root().await;
    let ctx = dice.updater(root).commit().await;

    let expected = (1 + 2 + 3) + (101 + 102 + 201 + 202) + 1000;
    assert_eq!(*ctx.compute(&Top).await?, expected);

    Ok(())
}
