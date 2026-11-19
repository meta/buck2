/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dice_futures::spawner::Spawner;
use dupe::Dupe;
use futures::future::BoxFuture;
use pagable::Pagable;
use pagable::pagable_typetag;
use tokio::task::JoinHandle;

use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::api::computations::DiceComputations;
use crate::api::cycles::DetectCycles;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::api::user_data::UserComputationData;
use crate::dice::Dice;

struct MySpawner(AtomicUsize);

impl<S> Spawner<S> for MySpawner {
    fn spawn(
        &self,
        _ctx: &S,
        fut: BoxFuture<'static, Box<dyn Any + Send + 'static>>,
    ) -> JoinHandle<Box<dyn Any + Send + 'static>> {
        self.0.fetch_add(1, Ordering::SeqCst);

        tokio::spawn(fut)
    }
}

#[derive(Allocative, Clone, Debug, Display, Eq, PartialEq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct K;

#[async_trait]
impl Key for K {
    type Value = ();

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| true)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[tokio::test]
async fn uses_custom_spawner() {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let spawner = Arc::new(MySpawner(AtomicUsize::new(0)));

    let mut data = UserComputationData::new();
    data.spawner = spawner.dupe();
    let updater = dice.updater_with_data(root, data);

    let ctx = updater.commit().await;

    ctx.compute(&K).await.unwrap();

    assert_eq!(spawner.0.load(Ordering::SeqCst), 1);
}
