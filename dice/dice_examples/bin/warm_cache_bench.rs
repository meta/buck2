/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Measures repeated resident `compute` calls in one transaction and context.
//! Compare optimized binaries from both revisions with identical arguments;
//! interleave their runs to reduce sensitivity to changing host load.

use std::sync::Arc;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use clap::Parser;
use clap::ValueEnum;
use derive_more::Display;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceStorage;
use dice::EqualityBehavior;
use dice::EvictionPolicy;
use dice::EvictionStrategy;
use dice::Key;
use dice::PagableValueSerialize;
use dice::ReclaimOptions;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;
use pagable::storage::noop::NoopPagableStorage;

#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("LeafKey({})", _0)]
#[pagable_typetag(dice::DiceKeyDyn)]
struct LeafKey(u32);

#[async_trait]
impl Key for LeafKey {
    type Value = u64;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        u64::from(self.0)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Paging {
    Disabled,
    All,
    Lru,
}

#[derive(Parser)]
struct Args {
    /// Number of distinct resident keys per worker.
    #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u32).range(1..))]
    num_keys: u32,
    /// Timed passes over the primed keys.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u32).range(1..))]
    warm_rounds: u32,
    /// Independent demand streams over disjoint keys, sharing one DICE engine.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    parallelism: u32,
    /// Measure paging bookkeeping without writing rows or evicting values.
    #[arg(long, value_enum, default_value_t = Paging::Disabled)]
    paging: Paging,
    /// Measure backed-candidate snapshot and classification without dropping protected values.
    #[arg(long, default_value_t = 0)]
    scan_rounds: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    anyhow::ensure!(
        args.scan_rounds == 0 || matches!(args.paging, Paging::Lru),
        "scan requires LRU"
    );
    let mut builder = Dice::builder();
    if !matches!(args.paging, Paging::Disabled) {
        let mut storage = DiceStorage::new(Arc::new(NoopPagableStorage::new()));
        storage.set_eviction_policy(match args.paging {
            Paging::Lru => EvictionPolicy::OnMemoryPressure(EvictionStrategy::Lru),
            _ => EvictionPolicy::OnMemoryPressure(EvictionStrategy::Default),
        });
        builder.set_pagable_storage(storage);
    }
    let dice = builder.build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    let tx = dice.updater(root).commit().await;
    let total_keys = args
        .num_keys
        .checked_mul(args.parallelism)
        .ok_or_else(|| anyhow::anyhow!("key count exceeds u32"))?;
    for i in 0..total_keys {
        tx.compute(&LeafKey(i)).await?;
    }

    if args.scan_rounds != 0 {
        drop(tx);
        dice.wait_for_idle().await;
        dice.page_out().await?;
        assert_eq!(dice.pagable_node_counts().await.backed, total_keys as usize);
        for sample in 0..=args.scan_rounds {
            let start = Instant::now();
            let evicted = dice
                .reclaim(
                    ReclaimOptions {
                        keep_recent_batches: 1,
                        ..ReclaimOptions::default()
                    },
                    || false,
                )
                .await;
            let elapsed = start.elapsed();
            assert_eq!(evicted, 0);
            println!(
                "{}",
                serde_json::json!({
                    "sample": sample,
                    "candidate_keys": total_keys,
                    "snapshot_and_classification_ns": elapsed.as_nanos(),
                })
            );
        }
        return Ok(());
    }

    let start = Instant::now();
    if args.parallelism == 1 {
        // Preserve the original benchmark loop for comparisons with older binaries.
        let mut ctx = tx.ctx();
        for _ in 0..args.warm_rounds {
            for i in 0..args.num_keys {
                ctx.compute(&LeafKey(i)).await?;
            }
        }
    } else {
        let mut workers = Vec::new();
        let barrier = Arc::new(tokio::sync::Barrier::new(args.parallelism as usize));
        for worker in 0..args.parallelism {
            let tx = tx.dupe();
            let barrier = barrier.dupe();
            workers.push(tokio::spawn(async move {
                let mut ctx = tx.ctx();
                let first = worker * args.num_keys;
                barrier.wait().await;
                for _ in 0..args.warm_rounds {
                    for i in first..first + args.num_keys {
                        ctx.compute(&LeafKey(i)).await?;
                    }
                }
                anyhow::Ok(())
            }));
        }
        for worker in workers {
            worker.await??;
        }
    }
    let elapsed = start.elapsed();
    let computes = u64::from(total_keys) * u64::from(args.warm_rounds);
    let ns_per_compute = elapsed.as_secs_f64() * 1e9 / computes as f64;
    println!(
        "{}",
        serde_json::json!({
            "computes": computes,
            "ns_per_compute": ns_per_compute,
            "parallelism": args.parallelism,
            "paging": format!("{:?}", args.paging),
        })
    );

    drop(tx);
    dice.wait_for_idle().await;
    Ok(())
}
