/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Creates a computation that consists of a top-chain, a bottom-chain, and a dense middle.
//!
//! The dependency structure is something like:
//!   T0 -> T1 -> T2 -> ... -> TN -> DN <...> D0
//!    |     |     |            |              |
//!    v     v     v            v              |
//!   B0 <- B1 <- B2 <- ... <- BN <------------/
//!    |
//!    v
//!   Leaf
//!
//! We use this computation to explore the costs of computing and especially recomputing
//! different graph structures and to understand how dice is cancelling nodes during
//! recomputation.
//!
//! The process is that first we compute T0 at v0.
//!
//! We will then invalidate Leaf and recompute T0, but with additional side-channel enforced ordering.
//!
//! Specifically, we add waits in the computes so that:
//! 1. for all K > 0, the compute call for BK+1 pauses until the compute call for TK has started
//! 2. for all K, the compute call for DK pauses until the compute call for TN has started (the last node in the top chain)
//!
//! This causes dice to go through a specific controlled flow:
//!
//! 1. T0 is requested and is dirtied, so request recompute of all deps of T0
//! 2. this potentially starts recompute of the entire graph, but only B0 can actually proceed
//! 3. B0 is recomputed and has changed, still nothing can progress
//! 4. A dep of T0 has changed and so it must be actually recomputed
//! 5. Potentially dice cancels the other in-progess computations of T0 deps (and so recursively the whole graph)
//! 6. start recomputing T0, this requests T1
//! 7. T1 is requested and is dirtied, so request recompute of all deps of T1
//! 8. basically repeat  (1) - (6) for T1
//! 9. repeat (7) - (8) for T2, ..., TN
//! 10. finally, once TN has started recomputing it will allow all the dense nodes to recompute once it requests them.
//!
//! So simplified it kinda looks like in sequence:
//!
//! check_deps(T0), compute(B0), compute(T0), check_deps(T1), compute(B1), ..., compute(D0..DN)
//!
//! If at (5) dice actually cancels the in-progress computations, you can actually end up with many times more
//! cancellations than there are nodes in the graph.
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use clap::Parser;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceData;
use dice::DiceKeyDyn;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice::UserComputationData;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use tokio::sync::Semaphore;

struct Latches {
    /// One latch for each Top/Bottom pair. Bottom(K + 1) will wait for latch K to be released (by Top(K)).
    chain_latches: Vec<Semaphore>,
    /// One latch for all dense nodes, this will be released by the last Top node in the chain.
    dense_latch: Semaphore,
}

impl Latches {
    fn new(max_chain: u32) -> Self {
        Self {
            chain_latches: (0..(max_chain + 1)) // chain goes to max_chain
                .map(|_| Semaphore::new(0))
                .collect(),
            dense_latch: Semaphore::new(0),
        }
    }

    async fn chain_latch_wait(&self, i: usize) {
        if let Some(v) = self.chain_latches.get(i) {
            v.acquire().await.unwrap().forget()
        }
    }

    fn chain_latch_release(&self, i: usize) {
        self.chain_latches
            .get(i)
            .unwrap()
            .add_permits(Semaphore::MAX_PERMITS / 10);
    }

    async fn dense_latch_wait(&self) {
        self.dense_latch.acquire().await.unwrap().forget();
    }

    fn dense_latch_release(&self) {
        self.dense_latch
            .add_permits(Semaphore::MAX_PERMITS - self.dense_latch.available_permits());
    }

    /// Releases all latches, we use this for the first computation.
    fn release_all(&self) {
        self.dense_latch_release();
        for i in 0..self.chain_latches.len() {
            self.chain_latch_release(i)
        }
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("TopKey({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
pub struct TopKey(u32);

#[async_trait]
impl Key for TopKey {
    type Value = u32;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> u32 {
        let config = ctx.compute(&ConfigKey).await.unwrap();
        let latches = ctx.per_transaction_data().data.get::<Latches>().unwrap();
        if self.0 == config.chain_count {
            latches.dense_latch_release();
            ctx.compute2(
                async |ctx| drop(ctx.compute(&DenseKey(config.dense_count)).await),
                async |ctx| drop(ctx.compute(&BottomKey(self.0)).await),
            )
            .await;
        } else {
            latches.chain_latch_release(self.0 as usize + 1);
            ctx.compute2(
                async |ctx| {
                    // This allows time to drop the graph of things below this before we re-request them.
                    std::thread::sleep(Duration::from_millis(config.wait_millis));
                    drop(ctx.compute(&TopKey(self.0 + 1)).await)
                },
                async |ctx| drop(ctx.compute(&BottomKey(self.0)).await),
            )
            .await;
        }
        self.0
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("BottomKey({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
pub struct BottomKey(u32);

#[async_trait]
impl Key for BottomKey {
    type Value = u32;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> u32 {
        let config = ctx.compute(&ConfigKey).await.unwrap();
        let latches = ctx.per_transaction_data().data.get::<Latches>().unwrap();
        latches.chain_latch_wait(self.0 as usize).await;
        // this gives time for the graph of deps to be all requested before returning a changed value and cancelling them
        std::thread::sleep(Duration::from_millis(config.wait_millis));
        if self.0 == 0 {
            drop(ctx.compute(&Leaf).await);
        } else {
            drop(ctx.compute(&BottomKey(self.0 - 1)).await);
        }
        self.0
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("DenseKey({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
pub struct DenseKey(u32);

#[async_trait]
impl Key for DenseKey {
    type Value = u32;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> u32 {
        let config = ctx.compute(&ConfigKey).await.unwrap();
        let latches = ctx.per_transaction_data().data.get::<Latches>().unwrap();
        latches.dense_latch_wait().await;
        std::thread::sleep(Duration::from_millis(config.wait_millis));
        if self.0 == 0 {
            drop(ctx.compute(&BottomKey(config.chain_count)).await)
        } else {
            drop(
                ctx.compute_join(0..self.0, async |ctx, v| {
                    drop(ctx.compute(&DenseKey(v)).await)
                })
                .await,
            );
        }
        self.0
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Leaf")]
#[pagable_typetag(DiceKeyDyn)]
pub struct Leaf;

impl InjectedKey for Leaf {
    type Value = u32;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("ConfigKey")]
#[pagable_typetag(DiceKeyDyn)]
pub struct ConfigKey;

impl InjectedKey for ConfigKey {
    type Value = Config;

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        dice::NoValueSerialize::<Self::Value>::new()
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|_x, _y| false)
    }
}

#[derive(Parser, Debug, Clone, Copy, Dupe, Allocative)]
pub struct Config {
    #[arg(long, default_value_t = 100)]
    chain_count: u32,
    #[arg(long, default_value_t = 1000)]
    dense_count: u32,
    #[arg(long, default_value_t = 5)]
    wait_millis: u64,
    #[arg(long, default_value_t = false)]
    modern: bool,
    #[arg(long, default_value_t = false)]
    detect_cycles: bool,
}

#[tokio::main]
async fn main() {
    let config = Config::parse();

    eprintln!("Using config {:?}", config);

    let builder = Dice::builder();
    let dice = if config.detect_cycles {
        builder.build(DetectCycles::Enabled)
    } else {
        builder.build(DetectCycles::Disabled)
    };
    let root = dice.new_root().await;

    let start = Instant::now();

    {
        let mut ctx = dice.updater(root);
        ctx.changed_to(vec![(ConfigKey, config)]).unwrap();
        ctx.commit().await;
    }

    eprintln!("starting first computation");

    {
        let mut dice_data = DiceData::new();
        let latches = Latches::new(config.chain_count);
        latches.release_all();
        dice_data.set(latches);
        let mut ctx = dice.updater_with_data(
            root,
            UserComputationData {
                data: dice_data,
                ..Default::default()
            },
        );

        ctx.changed_to(vec![(Leaf, 0)]).unwrap();

        let ctx = ctx.commit().await;
        drop(ctx.compute(&TopKey(0)).await);
    }

    let first_done = Instant::now();

    eprintln!("first computation took {}s", start.elapsed().as_secs_f32());

    let latches = Latches::new(config.chain_count);
    latches.chain_latch_release(0);

    {
        let mut dice_data = DiceData::new();
        dice_data.set(latches);
        let mut ctx = dice.updater_with_data(
            root,
            UserComputationData {
                data: dice_data,
                ..Default::default()
            },
        );
        ctx.changed_to(vec![(Leaf, 1)]).unwrap();
        let ctx = ctx.commit().await;
        drop(ctx.compute(&TopKey(0)).await);
    }

    eprintln!("recompute took {}s", first_done.elapsed().as_secs_f32());
}
