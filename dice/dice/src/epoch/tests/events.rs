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
use std::sync::Mutex;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::Dice;
use crate::DiceDataBuilder;
use crate::DiceEvent;
use crate::DiceEventListener;
use crate::DiceKeyDyn;
use crate::EqualityBehavior;
use crate::InjectedKey;
use crate::api::computations::DiceComputations;
use crate::api::cycles::DetectCycles;
use crate::api::key::Key;
use crate::api::key::NoValueSerialize;
use crate::api::key::ValueSerialize;
use crate::api::user_data::UserComputationData;

#[derive(Default, Allocative)]
struct Tracker {
    state: Mutex<Vec<DiceEvent>>,
}

impl DiceEventListener for Tracker {
    fn event(&self, event: DiceEvent) {
        self.state.lock().unwrap().push(event);
    }
}

#[derive(Clone, Dupe, Debug, Display, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Injected;

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

#[derive(Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Stage0;

#[async_trait]
impl Key for Stage0 {
    type Value = ();

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.compute(&Injected).await.unwrap();
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| true)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

#[derive(Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Allocative, Pagable)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct Stage1;

#[async_trait]
impl Key for Stage1 {
    type Value = ();

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        ctx.compute(&Stage0).await.unwrap();
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| true)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

async fn test_events_impl(builder: DiceDataBuilder) -> anyhow::Result<()> {
    let dice = builder.build(DetectCycles::Enabled);
    let root = dice.new_root().await;

    {
        let tracker = Arc::new(Tracker::default());

        let data = UserComputationData {
            tracker: tracker.dupe(),
            ..Default::default()
        };

        let mut updater = dice.updater_with_data(root, data);
        updater.changed_to(vec![(Injected, 123)])?;

        let transaction = updater.commit().await;
        transaction.compute(&Stage1).await?;

        assert_eq!(
            &*tracker.state.lock().unwrap(),
            &[
                DiceEvent::Started { key_type: "Stage1" },
                DiceEvent::ComputeStarted { key_type: "Stage1" },
                DiceEvent::Started { key_type: "Stage0" },
                DiceEvent::ComputeStarted { key_type: "Stage0" },
                DiceEvent::ComputeFinished { key_type: "Stage0" },
                DiceEvent::Finished { key_type: "Stage0" },
                DiceEvent::ComputeFinished { key_type: "Stage1" },
                DiceEvent::Finished { key_type: "Stage1" },
            ]
        );
    }

    {
        let tracker = Arc::new(Tracker::default());

        let data = UserComputationData {
            tracker: tracker.dupe(),
            ..Default::default()
        };

        // Change the value.
        let mut updater = dice.updater_with_data(root, data);
        updater.changed_to(vec![(Injected, 456)])?;

        let transaction = updater.commit().await;
        transaction.compute(&Stage1).await?;

        assert_eq!(
            &*tracker.state.lock().unwrap(),
            &[
                DiceEvent::Started { key_type: "Stage1" },
                DiceEvent::CheckDepsStarted { key_type: "Stage1" },
                DiceEvent::Started { key_type: "Stage0" },
                DiceEvent::CheckDepsStarted { key_type: "Stage0" },
                DiceEvent::CheckDepsFinished { key_type: "Stage0" },
                DiceEvent::ComputeStarted { key_type: "Stage0" },
                DiceEvent::ComputeFinished { key_type: "Stage0" },
                DiceEvent::Finished { key_type: "Stage0" },
                DiceEvent::CheckDepsFinished { key_type: "Stage1" },
                DiceEvent::Finished { key_type: "Stage1" },
            ]
        );
    }

    Ok(())
}

#[tokio::test]
async fn test_events_modern() -> anyhow::Result<()> {
    test_events_impl(Dice::builder()).await
}
