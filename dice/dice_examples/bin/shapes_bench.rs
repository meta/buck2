/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Baseline benchmark suite for the public dice API.
//!
//! A fixed set of small, deterministic scenarios covering:
//!
//! - Cold compute on three graph shapes (chain, wide fanout, layered DAG).
//! - Incremental recompute where the invalidation either propagates end-to-end
//!   or dies at a value-equality cutoff.
//! - `changed_to` sweep across many independent injected keys.
//! - High-fanout invalidation of a single injected key with many dependents.
//!
//! Every scenario uses a fresh `Dice` and touches only the crate's public
//! surface. Sizes are picked so the whole suite finishes in a few seconds.
//!
//! Output goes to stderr as `key = value` lines, one per scenario, plus a
//! header naming the machine and worker count so numbers from different
//! machines don't get compared blindly.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice::BranchId;
use dice::DetectCycles;
use dice::Dice;
use dice::DiceComputations;
use dice::DiceKeyDyn;
use dice::EqualityBehavior;
use dice::InjectedKey;
use dice::Key;
use dice::NoValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::Pagable;
use pagable::pagable_typetag;

// -- Sizes ---------------------------------------------------------------

const CHAIN_LEN: u32 = 500;
const WIDE_W: u32 = 1000;
const LAYER_DEPTH: u16 = 8;
const LAYER_WIDTH: u32 = 64;
const LAYER_FANIN: u32 = 4;
const SWEEP_N: u32 = 500;
const RECOMPUTE_ITERS: u32 = 20;
const WIDE_RECOMPUTE_ITERS: u32 = 10;
const SWEEP_ITERS: u32 = 10;

// -- Keys ----------------------------------------------------------------
//
// Every computed key uses `NoValueSerialize` because none of these scenarios
// exercise dice's paging path. All keys share the invariant that their
// `compute` bodies are pure functions of their dependencies, so equality
// propagation is well-defined.

/// Shared injected leaf. Some scenarios use only index 0; the sweep uses
/// indices 0..SWEEP_N.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Leaf({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct Leaf(u32);

impl InjectedKey for Leaf {
    type Value = u64;

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Reads `Leaf(0)` but always returns 0, so downstream keys see the leaf
/// change as a no-op after `Absorb` recomputes. Used to terminate the
/// early-cutoff chain / wide scenarios.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Absorb")]
#[pagable_typetag(DiceKeyDyn)]
struct Absorb;

#[async_trait]
impl Key for Absorb {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        ctx.compute(&Leaf(0))
            .await
            .expect("benchmark keys never fail");
        0
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Chain node whose value is `Leaf(0) + (CHAIN_LEN - i)`, so a change in the
/// leaf propagates through every link.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("ChainFull({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct ChainFull(u32);

#[async_trait]
impl Key for ChainFull {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let dep = if self.0 + 1 < CHAIN_LEN {
            *ctx.compute(&ChainFull(self.0 + 1))
                .await
                .expect("benchmark keys never fail")
        } else {
            *ctx.compute(&Leaf(0))
                .await
                .expect("benchmark keys never fail")
        };
        dep + 1
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Chain terminated by `Absorb`, so a leaf change is dropped at the last
/// link and the rest of the chain revalidates without recomputing.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("ChainCutoff({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct ChainCutoff(u32);

#[async_trait]
impl Key for ChainCutoff {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        if self.0 + 1 < CHAIN_LEN {
            *ctx.compute(&ChainCutoff(self.0 + 1))
                .await
                .expect("benchmark keys never fail")
        } else {
            *ctx.compute(&Absorb)
                .await
                .expect("benchmark keys never fail")
        }
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// One of the WIDE_W parallel branches for the full-propagation wide
/// scenario. Value depends directly on the leaf.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("WideFullLeaf({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct WideFullLeaf(u32);

#[async_trait]
impl Key for WideFullLeaf {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let leaf = *ctx
            .compute(&Leaf(0))
            .await
            .expect("benchmark keys never fail");
        leaf.wrapping_add(self.0 as u64)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Root for the full-propagation wide scenario. Fans out to all WideFullLeaf
/// branches and sums their values, so any leaf change reaches the root.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("WideFullRoot")]
#[pagable_typetag(DiceKeyDyn)]
struct WideFullRoot;

#[async_trait]
impl Key for WideFullRoot {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let values = ctx
            .compute_join(0..WIDE_W, async |ctx, i| {
                *ctx.compute(&WideFullLeaf(i))
                    .await
                    .expect("benchmark keys never fail")
            })
            .await;
        values.into_iter().fold(0u64, u64::wrapping_add)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Cutoff variant of WideFullLeaf: reads the leaf indirectly through
/// `Absorb`, so the leaf change dies at Absorb and each branch revalidates
/// without recomputing.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("WideCutoffLeaf({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct WideCutoffLeaf(u32);

#[async_trait]
impl Key for WideCutoffLeaf {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let absorbed = *ctx
            .compute(&Absorb)
            .await
            .expect("benchmark keys never fail");
        absorbed.wrapping_add(self.0 as u64)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Root for the cutoff wide scenario.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("WideCutoffRoot")]
#[pagable_typetag(DiceKeyDyn)]
struct WideCutoffRoot;

#[async_trait]
impl Key for WideCutoffRoot {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let values = ctx
            .compute_join(0..WIDE_W, async |ctx, i| {
                *ctx.compute(&WideCutoffLeaf(i))
                    .await
                    .expect("benchmark keys never fail")
            })
            .await;
        values.into_iter().fold(0u64, u64::wrapping_add)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Node in a layered DAG. Layer 0 depends directly on the leaf. Higher
/// layers depend on `LAYER_FANIN` deterministically-chosen nodes from the
/// previous layer, so each higher-layer node aggregates over a distinct
/// slice.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Layer({}, {})", _0, _1)]
#[pagable_typetag(DiceKeyDyn)]
struct LayerNode(u16, u32);

#[async_trait]
impl Key for LayerNode {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let (depth, idx) = (self.0, self.1);
        if depth == 0 {
            let leaf = *ctx
                .compute(&Leaf(0))
                .await
                .expect("benchmark keys never fail");
            leaf.wrapping_add(idx as u64)
        } else {
            let prev = depth - 1;
            let values = ctx
                .compute_join(0..LAYER_FANIN, async |ctx, j| {
                    let dep_idx = (idx.wrapping_mul(LAYER_FANIN).wrapping_add(j)) % LAYER_WIDTH;
                    *ctx.compute(&LayerNode(prev, dep_idx))
                        .await
                        .expect("benchmark keys never fail")
                })
                .await;
            values.into_iter().fold(0u64, u64::wrapping_add)
        }
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Root over the top layer of the layered DAG.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("LayerRoot")]
#[pagable_typetag(DiceKeyDyn)]
struct LayerRoot;

#[async_trait]
impl Key for LayerRoot {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let top = LAYER_DEPTH - 1;
        let values = ctx
            .compute_join(0..LAYER_WIDTH, async |ctx, i| {
                *ctx.compute(&LayerNode(top, i))
                    .await
                    .expect("benchmark keys never fail")
            })
            .await;
        values.into_iter().fold(0u64, u64::wrapping_add)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// One node in the injected-sweep graph. Reads `Leaf(i)` (a distinct
/// injected key per node) so bumping every leaf in one commit dirties every
/// SweepNode independently.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("SweepNode({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct SweepNode(u32);

#[async_trait]
impl Key for SweepNode {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let v = *ctx
            .compute(&Leaf(self.0))
            .await
            .expect("benchmark keys never fail");
        v.wrapping_add(self.0 as u64)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

/// Root over the injected-sweep subgraph.
#[derive(Clone, Display, Debug, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("SweepRoot")]
#[pagable_typetag(DiceKeyDyn)]
struct SweepRoot;

#[async_trait]
impl Key for SweepRoot {
    type Value = u64;

    async fn compute(&self, ctx: &mut DiceComputations, _c: &CancellationContext) -> u64 {
        let values = ctx
            .compute_join(0..SWEEP_N, async |ctx, i| {
                *ctx.compute(&SweepNode(i))
                    .await
                    .expect("benchmark keys never fail")
            })
            .await;
        values.into_iter().fold(0u64, u64::wrapping_add)
    }

    fn equality_behavior() -> EqualityBehavior<u64> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = u64> {
        NoValueSerialize::<u64>::new()
    }
}

// -- Timing helpers ------------------------------------------------------

struct Stats {
    min: Duration,
    median: Duration,
    max: Duration,
    iters: u32,
}

fn stats(durations: &mut [Duration]) -> Stats {
    assert!(!durations.is_empty());
    durations.sort();
    Stats {
        min: *durations.first().expect("at least one iteration"),
        median: durations[durations.len() / 2],
        max: *durations.last().expect("at least one iteration"),
        iters: durations.len() as u32,
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn print_cold(name: &str, params: &str, d: Duration) {
    eprintln!("{name:<32} {params:<26} cold={:>9.3} ms", ms(d));
}

/// Two aligned lines per scenario, one per phase. Grep by scenario name to
/// see both phases together, or by `phase=commit` / `phase=compute` to
/// isolate one across the whole run.
fn print_two_phase(name: &str, params: &str, commit: &Stats, compute: &Stats) {
    eprintln!(
        "{name:<32} {params:<26} iters={:<3} phase=commit  min={:>8.3} med={:>8.3} max={:>8.3} ms",
        commit.iters,
        ms(commit.min),
        ms(commit.median),
        ms(commit.max),
    );
    eprintln!(
        "{name:<32} {params:<26} iters={:<3} phase=compute min={:>8.3} med={:>8.3} max={:>8.3} ms",
        compute.iters,
        ms(compute.min),
        ms(compute.median),
        ms(compute.max),
    );
}

async fn seed_leaf_zero(dice: &Arc<Dice>, root: BranchId) {
    let mut updater = dice.updater(root);
    updater
        .changed_to(vec![(Leaf(0), 0u64)])
        .expect("benchmark inputs are always injectable");
    updater.commit().await;
}

async fn new_dice() -> (Arc<Dice>, BranchId) {
    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;
    (dice, root)
}

// -- Scenarios -----------------------------------------------------------

async fn bench_cold_chain() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    let ctx = dice.updater(root).commit().await;
    let start = Instant::now();
    ctx.compute(&ChainFull(0))
        .await
        .expect("benchmark keys never fail");
    let d = start.elapsed();
    print_cold("cold_chain", &format!("N={CHAIN_LEN}"), d);
}

async fn bench_cold_wide() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    let ctx = dice.updater(root).commit().await;
    let start = Instant::now();
    ctx.compute(&WideFullRoot)
        .await
        .expect("benchmark keys never fail");
    let d = start.elapsed();
    print_cold("cold_wide", &format!("W={WIDE_W}"), d);
}

async fn bench_cold_layered() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    let ctx = dice.updater(root).commit().await;
    let start = Instant::now();
    ctx.compute(&LayerRoot)
        .await
        .expect("benchmark keys never fail");
    let d = start.elapsed();
    print_cold(
        "cold_layered",
        &format!("D={LAYER_DEPTH} W={LAYER_WIDTH} F={LAYER_FANIN}"),
        d,
    );
}

async fn bench_recompute_chain_full() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    // Warm.
    let ctx = dice.updater(root).commit().await;
    ctx.compute(&ChainFull(0))
        .await
        .expect("benchmark keys never fail");
    drop(ctx);

    let mut commits = Vec::with_capacity(RECOMPUTE_ITERS as usize);
    let mut computes = Vec::with_capacity(RECOMPUTE_ITERS as usize);
    for i in 1..=RECOMPUTE_ITERS {
        let mut updater = dice.updater(root);
        updater
            .changed_to(vec![(Leaf(0), i as u64)])
            .expect("benchmark inputs are always injectable");
        // Commit does dice's invalidation walk; break it out so rewrites
        // that shift work between commit and compute are visible.
        let t0 = Instant::now();
        let ctx = updater.commit().await;
        commits.push(t0.elapsed());
        let t1 = Instant::now();
        ctx.compute(&ChainFull(0))
            .await
            .expect("benchmark keys never fail");
        computes.push(t1.elapsed());
    }
    print_two_phase(
        "recompute_chain_full",
        &format!("N={CHAIN_LEN}"),
        &stats(&mut commits),
        &stats(&mut computes),
    );
}

async fn bench_recompute_chain_cutoff() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    let ctx = dice.updater(root).commit().await;
    ctx.compute(&ChainCutoff(0))
        .await
        .expect("benchmark keys never fail");
    drop(ctx);

    let mut commits = Vec::with_capacity(RECOMPUTE_ITERS as usize);
    let mut computes = Vec::with_capacity(RECOMPUTE_ITERS as usize);
    for i in 1..=RECOMPUTE_ITERS {
        let mut updater = dice.updater(root);
        // The value under Leaf changes but Absorb collapses it back to 0,
        // so each iteration triggers a full revalidation walk without any
        // actual chain recompute.
        updater
            .changed_to(vec![(Leaf(0), i as u64)])
            .expect("benchmark inputs are always injectable");
        let t0 = Instant::now();
        let ctx = updater.commit().await;
        commits.push(t0.elapsed());
        let t1 = Instant::now();
        ctx.compute(&ChainCutoff(0))
            .await
            .expect("benchmark keys never fail");
        computes.push(t1.elapsed());
    }
    print_two_phase(
        "recompute_chain_cutoff",
        &format!("N={CHAIN_LEN}"),
        &stats(&mut commits),
        &stats(&mut computes),
    );
}

async fn bench_recompute_wide_full() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    let ctx = dice.updater(root).commit().await;
    ctx.compute(&WideFullRoot)
        .await
        .expect("benchmark keys never fail");
    drop(ctx);

    let mut commits = Vec::with_capacity(WIDE_RECOMPUTE_ITERS as usize);
    let mut computes = Vec::with_capacity(WIDE_RECOMPUTE_ITERS as usize);
    for i in 1..=WIDE_RECOMPUTE_ITERS {
        let mut updater = dice.updater(root);
        updater
            .changed_to(vec![(Leaf(0), i as u64)])
            .expect("benchmark inputs are always injectable");
        let t0 = Instant::now();
        let ctx = updater.commit().await;
        commits.push(t0.elapsed());
        let t1 = Instant::now();
        ctx.compute(&WideFullRoot)
            .await
            .expect("benchmark keys never fail");
        computes.push(t1.elapsed());
    }
    // Also covers the "high-fanout invalidation, full recompute" case: one
    // injected key with WIDE_W direct dependents.
    print_two_phase(
        "recompute_wide_full",
        &format!("W={WIDE_W}"),
        &stats(&mut commits),
        &stats(&mut computes),
    );
}

async fn bench_recompute_wide_cutoff() {
    let (dice, root) = new_dice().await;
    seed_leaf_zero(&dice, root).await;
    let ctx = dice.updater(root).commit().await;
    ctx.compute(&WideCutoffRoot)
        .await
        .expect("benchmark keys never fail");
    drop(ctx);

    let mut commits = Vec::with_capacity(WIDE_RECOMPUTE_ITERS as usize);
    let mut computes = Vec::with_capacity(WIDE_RECOMPUTE_ITERS as usize);
    for i in 1..=WIDE_RECOMPUTE_ITERS {
        let mut updater = dice.updater(root);
        updater
            .changed_to(vec![(Leaf(0), i as u64)])
            .expect("benchmark inputs are always injectable");
        let t0 = Instant::now();
        let ctx = updater.commit().await;
        commits.push(t0.elapsed());
        let t1 = Instant::now();
        ctx.compute(&WideCutoffRoot)
            .await
            .expect("benchmark keys never fail");
        computes.push(t1.elapsed());
    }
    // Also covers the "high-fanout invalidation, early cutoff" case.
    print_two_phase(
        "recompute_wide_cutoff",
        &format!("W={WIDE_W}"),
        &stats(&mut commits),
        &stats(&mut computes),
    );
}

async fn bench_injected_sweep() {
    let (dice, root) = new_dice().await;
    // Seed all SWEEP_N leaves.
    let initial: Vec<(Leaf, u64)> = (0..SWEEP_N).map(|i| (Leaf(i), 0u64)).collect();
    let mut updater = dice.updater(root);
    updater
        .changed_to(initial)
        .expect("benchmark inputs are always injectable");
    let ctx = updater.commit().await;
    ctx.compute(&SweepRoot)
        .await
        .expect("benchmark keys never fail");
    drop(ctx);

    let mut commits = Vec::with_capacity(SWEEP_ITERS as usize);
    let mut computes = Vec::with_capacity(SWEEP_ITERS as usize);
    for iter in 1..=SWEEP_ITERS {
        let updates: Vec<(Leaf, u64)> = (0..SWEEP_N).map(|i| (Leaf(i), iter as u64)).collect();
        let mut updater = dice.updater(root);
        updater
            .changed_to(updates)
            .expect("benchmark inputs are always injectable");
        let t0 = Instant::now();
        let ctx = updater.commit().await;
        commits.push(t0.elapsed());
        let t1 = Instant::now();
        ctx.compute(&SweepRoot)
            .await
            .expect("benchmark keys never fail");
        computes.push(t1.elapsed());
    }
    print_two_phase(
        "injected_sweep",
        &format!("N={SWEEP_N}"),
        &stats(&mut commits),
        &stats(&mut computes),
    );
}

// -- Driver --------------------------------------------------------------

#[tokio::main]
async fn main() {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    eprintln!("dice shapes_bench  workers={workers}  (numbers are wall time; single machine)");
    eprintln!();

    bench_cold_chain().await;
    bench_cold_wide().await;
    bench_cold_layered().await;
    bench_recompute_chain_full().await;
    bench_recompute_chain_cutoff().await;
    bench_recompute_wide_full().await;
    bench_recompute_wide_cutoff().await;
    bench_injected_sweep().await;
}
