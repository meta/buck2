/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use pagable::Pagable;
use pagable::pagable_typetag;
use tokio::sync::oneshot;

use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::cycles::DetectCycles;
use crate::api::injected::InjectedKey;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::dice::Dice;

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{}", self.0)]
#[pagable_typetag(DiceKeyDyn)]
struct Injected(i32);

#[async_trait]
impl InjectedKey for Injected {
    type Value = i32;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("SpawnedKey")]
#[pagable_typetag(DiceKeyDyn)]
struct SpawnedKey;

#[async_trait]
impl Key for SpawnedKey {
    type Value = i32;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.spawned(|ctx, _cancellation| {
            async move { *ctx.compute(&Injected(0)).await.unwrap() }.boxed()
        })
        .await
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// Test that spawned can compute a value
#[tokio::test]
async fn spawned_basic() -> anyhow::Result<()> {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Injected(0), 42)])?;
    let ctx = updater.commit().await;

    let result = ctx.compute(&SpawnedKey).await?;
    assert_eq!(*result, 42);

    Ok(())
}

/// Test that deps from spawned tasks are tracked
#[tokio::test]
async fn spawned_tracks_deps() -> anyhow::Result<()> {
    static COMPUTE_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("CountingKey")]
    #[pagable_typetag(DiceKeyDyn)]
    struct CountingKey;

    #[async_trait]
    impl Key for CountingKey {
        type Value = i32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            COMPUTE_COUNT.fetch_add(1, Ordering::SeqCst);
            ctx.spawned(|ctx, _cancellation| {
                async move { *ctx.compute(&Injected(0)).await.unwrap() }.boxed()
            })
            .await
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

    // Initial computation
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Injected(0), 100)])?;
    let ctx = updater.commit().await;

    let result = ctx.compute(&CountingKey).await?;
    assert_eq!(*result, 100);
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 1);

    // Same value, should be cached
    let ctx = dice.updater(root).commit().await;
    let result = ctx.compute(&CountingKey).await?;
    assert_eq!(*result, 100);
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 1);

    // Change the dep, should recompute
    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Injected(0), 200)])?;
    let ctx = updater.commit().await;

    let result = ctx.compute(&CountingKey).await?;
    assert_eq!(*result, 200);
    assert_eq!(COMPUTE_COUNT.load(Ordering::SeqCst), 2);

    Ok(())
}

/// Test multiple spawned tasks
#[tokio::test]
async fn spawned_multiple() -> anyhow::Result<()> {
    #[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[display("MultiSpawnKey")]
    #[pagable_typetag(DiceKeyDyn)]
    struct MultiSpawnKey;

    #[async_trait]
    impl Key for MultiSpawnKey {
        type Value = i32;

        async fn compute(
            &self,
            ctx: &mut DiceComputations,
            _cancellations: &CancellationContext,
        ) -> Self::Value {
            let a = ctx
                .spawned(|ctx, _cancellation| {
                    async move { *ctx.compute(&Injected(0)).await.unwrap() }.boxed()
                })
                .await;

            let b = ctx
                .spawned(|ctx, _cancellation| {
                    async move { *ctx.compute(&Injected(1)).await.unwrap() }.boxed()
                })
                .await;

            a + b
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
    updater.changed_to(vec![(Injected(0), 10), (Injected(1), 20)])?;
    let ctx = updater.commit().await;

    let result = ctx.compute(&MultiSpawnKey).await?;
    assert_eq!(*result, 30);

    Ok(())
}

/// Test that spawned task starts running without awaiting the returned future
#[tokio::test]
async fn spawned_runs_without_await() -> anyhow::Result<()> {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    let mut updater = dice.updater(root);
    updater.changed_to(vec![(Injected(0), 42)])?;
    let ctx = updater.commit().await;
    let mut ctx = ctx.ctx();

    let (tx, rx) = oneshot::channel::<i32>();

    // Spawned task should start running immediately, even before we await
    let _future = ctx.spawned(|ctx, _cancellation| {
        async move {
            let val = *ctx.compute(&Injected(0)).await.unwrap();
            let _ = tx.send(val);
        }
        .boxed()
    });

    // Wait on the oneshot instead of the spawned future - this verifies the
    // task is actually running independently
    let result = tokio::time::timeout(Duration::from_secs(5), rx).await??;
    assert_eq!(result, 42);

    Ok(())
}
