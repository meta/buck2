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

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::join3;
use pagable::PagablePanic;
use pagable::pagable_typetag;
use tokio::sync::Mutex;

use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::data::DiceData;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::dice::Dice;

#[tokio::test]
async fn concurrent_identical_requests_are_deduped() -> anyhow::Result<()> {
    #[derive(Allocative, Clone, Debug, Display, PagablePanic)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    struct ComputeOnce(#[allocative(skip)] Arc<Mutex<u8>>);

    impl PartialEq for ComputeOnce {
        fn eq(&self, _other: &Self) -> bool {
            true
        }
    }

    impl Eq for ComputeOnce {}

    impl Hash for ComputeOnce {
        fn hash<H: Hasher>(&self, _state: &mut H) {}
    }

    #[async_trait]
    impl Key for ComputeOnce {
        type Value = ();

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            let mut count = self.0.lock().await;
            *count += 1;
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let dice = Dice::testing_new(DiceData::new());
    let root = dice.new_root().await;

    let guard = Arc::new(Mutex::new(0));
    let _g = guard.lock().await;

    let ctx = dice.updater(root).commit().await.0;
    let mut ctx = ctx.as_computations();

    let k = &ComputeOnce(guard.dupe());

    let (compute1, compute2, compute3) = ctx.compute3(
        |ctx| ctx.compute(k).boxed(),
        |ctx| ctx.compute(k).boxed(),
        |ctx| ctx.compute(k).boxed(),
    );

    drop(_g);

    let (r1, r2, r3) = join3(compute1, compute2, compute3).await;
    r1?;
    r2?;
    r3?;

    assert_eq!(1, *guard.lock().await);

    Ok(())
}

#[test]
fn different_requests_are_spawned_in_parallel() -> anyhow::Result<()> {
    let n_thread = 10usize;

    #[derive(Allocative, Clone, Debug, Display, PagablePanic)]
    #[display("{:?}", self)]
    #[pagable_typetag(DiceKeyDyn)]
    // purposely use a sync barrier to see that our compute is spawned
    struct ComputeParallel(#[allocative(skip)] Arc<std::sync::Barrier>);

    impl PartialEq for ComputeParallel {
        fn eq(&self, _other: &Self) -> bool {
            false
        }
    }

    impl Eq for ComputeParallel {}

    impl Hash for ComputeParallel {
        fn hash<H: Hasher>(&self, _state: &mut H) {}
    }

    #[async_trait]
    impl Key for ComputeParallel {
        type Value = usize;

        async fn compute(
            &self,
            _ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            self.0.wait();

            1
        }

        fn equality_behavior() -> EqualityBehavior<Self::Value> {
            EqualityBehavior::Compare(|_x, _y| true)
        }

        fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
            NoValueSerialize::<Self::Value>::new()
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(n_thread)
        .enable_time()
        .build()
        .unwrap();

    // use barrier to ensure that n_threads in parallel are spawned by the engine
    let barrier = Arc::new(std::sync::Barrier::new(n_thread));

    rt.block_on(async move {
        let dice = Dice::testing_new(DiceData::new());
        let root = dice.new_root().await;

        let ctx = &dice.updater(root).commit().await;
        let k = &ComputeParallel(barrier.dupe());

        let futs = (0..n_thread)
            .map(|_| async move { ctx.clone().compute(k).await.copied() })
            .collect::<Vec<_>>();

        let mut sum = 0;
        dice_futures::join::join_all(futs)
            .await
            .into_iter()
            .try_for_each(|res| {
                sum += res?;
                anyhow::Ok(())
            })?;

        assert_eq!(sum, n_thread);

        anyhow::Ok::<()>(())
    })?;

    Ok(())
}
