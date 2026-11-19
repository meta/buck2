/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg(test)]

use std::collections::HashMap;
use std::sync::Arc;

use allocative::Allocative;
use async_trait::async_trait;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceData;
use dice::DiceKeyDyn;
use dice::DiceProjectionComputations;
use dice::DiceProjectionDyn;
use dice::EqualityBehavior;
use dice::Key;
use dice::ProjectionKey;
use dice::UserComputationData;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use parking_lot::Mutex;

/// We have three keys in this test:
/// * key for a global "configuration"
/// * key for a configuration "property"
/// * key for a "file" which reads a "property" from a "configuration"
///
/// This enum describes types of these keys.
#[derive(PartialEq, Debug)]
enum Computation {
    File,
    Config,
    ConfigProperty,
}

/// Produce side effects during computation of each key.
/// Of course, users should not do that, but we are testing internals.
struct RecordedComputations {
    computations: Vec<Computation>,
}

/// This is what "configuration" key reads from the outside world.
struct GlobalConfig {
    config: HashMap<String, String>,
}

/// "Evaluate" a file.
#[derive(
    Debug,
    derive_more::Display,
    Clone,
    Hash,
    PartialEq,
    Eq,
    Allocative,
    Pagable
)]
#[display("{}", name)]
#[pagable_typetag(DiceKeyDyn)]
struct FileKey {
    name: String,
}

#[async_trait]
impl Key for FileKey {
    type Value = Result<Arc<String>, Arc<anyhow::Error>>;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        // Read "config".
        let config = ctx
            .compute_opaque(&ConfigKey)
            .await
            .map_err(|e| Arc::new(anyhow::anyhow!(e)))?;
        // But use only one "property" of the "config",
        // which is the result of file evaluation.
        // We are testing that file evaluation is not invalidated
        // if unrelated configurations changed.
        let value = ctx
            .projection(
                &config,
                &ConfigPropertyKey {
                    key: "x".to_owned(),
                },
            )
            .map_err(|e| Arc::new(anyhow::anyhow!(e)))?;
        // Record we executed this computation.
        ctx.global_data()
            .get::<Arc<Mutex<RecordedComputations>>>()
            .unwrap()
            .lock()
            .computations
            .push(Computation::File);
        Ok(Arc::new(format!("<{value}>")))
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| unreachable!("not used in test"))
    }
}

/// Global "configuration".
#[derive(
    Debug,
    derive_more::Display,
    Clone,
    Dupe,
    Hash,
    PartialEq,
    Eq,
    Allocative,
    Pagable
)]
#[display("{:?}", self)]
#[pagable_typetag(DiceKeyDyn)]
struct ConfigKey;

#[async_trait]
impl Key for ConfigKey {
    type Value = Arc<HashMap<String, String>>;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Arc<HashMap<String, String>> {
        // Record we performed this computation.
        ctx.global_data()
            .get::<Arc<Mutex<RecordedComputations>>>()
            .unwrap()
            .lock()
            .computations
            .push(Computation::Config);
        // And produce a value fetched from the outside world.
        Arc::new(
            ctx.per_transaction_data()
                .data
                .get::<GlobalConfig>()
                .unwrap()
                .config
                .clone(),
        )
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

/// One "property" of the "configuration".
#[derive(
    Debug,
    derive_more::Display,
    Clone,
    Hash,
    PartialEq,
    Eq,
    Allocative,
    Pagable
)]
#[display("{}", key)]
#[pagable_typetag(DiceProjectionDyn)]
struct ConfigPropertyKey {
    key: String,
}

impl ProjectionKey for ConfigPropertyKey {
    /// We read a property from the config.
    type DeriveFromKey = ConfigKey;
    /// And produce a string.
    type Value = Arc<String>;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn compute(
        &self,
        derive_from: &Arc<HashMap<String, String>>,
        ctx: &DiceProjectionComputations,
    ) -> Arc<String> {
        // Record we performed this computation.
        ctx.global_data()
            .get::<Arc<Mutex<RecordedComputations>>>()
            .unwrap()
            .lock()
            .computations
            .push(Computation::ConfigProperty);
        // Fetch the config property.
        let value = derive_from
            .get(&self.key)
            .map_or_else(|| "NO".to_owned(), |x| x.to_owned());
        Arc::new(value)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
}

#[tokio::test]
async fn smoke() -> anyhow::Result<()> {
    let tracker = Arc::new(Mutex::new(RecordedComputations {
        computations: Vec::new(),
    }));

    let mut dice = Dice::builder();
    dice.set(tracker.dupe());
    let dice = dice.build(DetectCycles::Enabled);
    let root = dice.new_root().await;

    // Part 1: full evaluation. We request a file,
    // and dice evaluates: config -> config property -> file.

    let mut data = DiceData::new();
    data.set(GlobalConfig {
        config: HashMap::from_iter([("x".to_owned(), "X".to_owned())]),
    });
    let ctx = dice.updater_with_data(
        root,
        UserComputationData {
            data,
            ..Default::default()
        },
    );

    let ctx = ctx.commit().await;

    let file = ctx
        .compute(&FileKey {
            name: "file.fl".to_owned(),
        })
        .await?
        .clone()
        .map_err(|e| anyhow::anyhow!(format!("{:#}", e)))?;
    assert_eq!("<X>", &*file);

    assert_eq!(
        [
            Computation::Config,
            Computation::ConfigProperty,
            Computation::File
        ]
        .as_slice(),
        tracker.lock().computations.as_slice()
    );

    let mut ctx = dice.updater(root);
    ctx.changed([ConfigKey])?;
    ctx.commit().await;
    tracker.lock().computations.clear();

    // Part 2: we update the config with the identical config.
    // Dice performs only "config" computation,
    // and the rest remains cached.

    let mut data = UserComputationData::new();
    data.data.set(GlobalConfig {
        config: HashMap::from_iter([("x".to_owned(), "X".to_owned())]),
    });
    let ctx = dice.updater_with_data(root, data).commit().await;

    let file = ctx
        .compute(&FileKey {
            name: "file.fl".to_owned(),
        })
        .await?
        .clone()
        .map_err(|e| anyhow::anyhow!(format!("{:#}", e)))?;
    assert_eq!("<X>", &*file);

    assert_eq!(
        [Computation::Config].as_slice(),
        tracker.lock().computations.as_slice()
    );

    let mut ctx = dice.updater(root);
    ctx.changed([ConfigKey])?;
    ctx.commit().await;
    tracker.lock().computations.clear();

    // Part 3: we update the config with a different config,
    // which however preserves the config property we are interested in.
    // So dice performs "config" and "config property" computations,
    // but since "config property" result is unchanged, "file" is not reevaluated.

    let mut data = UserComputationData::new();
    data.data.set(GlobalConfig {
        config: HashMap::from_iter([
            ("x".to_owned(), "X".to_owned()),
            ("y".to_owned(), "Y".to_owned()),
        ]),
    });
    let ctx = dice.updater_with_data(root, data).commit().await;

    let file = ctx
        .compute(&FileKey {
            name: "file.fl".to_owned(),
        })
        .await?
        .clone()
        .map_err(|e| anyhow::anyhow!(format!("{:#}", e)))?;
    assert_eq!("<X>", &*file);

    assert_eq!(
        [Computation::Config, Computation::ConfigProperty].as_slice(),
        tracker.lock().computations.as_slice()
    );

    Ok(())
}
