/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Stress test for DICE graph invalidation that doesn't require recomputation.
//!
//! Scenario:
//! 1. A `Seed` (InjectedKey) changes each iteration
//! 2. A `Buffer` key depends on Seed but always produces the same value (0)
//! 3. A large graph of `GraphNode` keys transitively depends on Buffer
//!
//! When Seed changes, DICE must walk the entire dirty graph to verify that
//! nothing actually needs recomputation (because Buffer absorbs the change).
//! This measures the cost of that invalidation walk.
//!
//! Supports multiple graph shapes via `--shape`:
//! - `wide`:  Root depends on N-1 leaf nodes, each depending on Buffer
//! - `chain`: Linear chain of N nodes ending at Buffer
//! - `tree`:  K-ary tree of depth D, leaves depend on Buffer
//! - `dense`: DAG where node i depends on nodes j in (i, i + dense_width], last node depends on
//!            Buffer. With dense_width=0 (default), node i depends on all j > i.
//!
//! The `--shadows` parameter creates multiple independent copies of the graph
//! that share the same Seed and Buffer keys. This multiplies the total number
//! of nodes without changing the graph shape.

use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use async_trait::async_trait;
use clap::Parser;
use clap::ValueEnum;
use derive_more::Display;
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

static ARGS: OnceLock<Args> = OnceLock::new();

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Shape {
    /// Root depends on N-1 leaf nodes, each depending on Buffer.
    Wide,
    /// Linear chain: node 0 -> 1 -> ... -> N-1 -> Buffer.
    Chain,
    /// K-ary tree of depth D. Leaves depend on Buffer.
    Tree,
    /// Dense DAG: node i depends on all j where i < j < i + dense_width.
    /// Node N-1 depends on Buffer.
    Dense,
}

#[derive(Parser, Debug, Clone, Copy)]
struct Args {
    /// Graph shape to stress test.
    #[arg(long, value_enum, default_value_t = Shape::Dense)]
    shape: Shape,

    /// Number of graph nodes (for wide, chain, dense).
    #[arg(long, default_value_t = 1000)]
    node_count: u32,

    /// Branching factor (for tree).
    #[arg(long, default_value_t = 4)]
    branching: u32,

    /// Tree depth, i.e. number of edges from root to leaf (for tree).
    #[arg(long, default_value_t = 6)]
    depth: u32,

    /// Number of invalidation iterations.
    #[arg(long, default_value_t = 10)]
    iterations: u32,

    /// Enable DICE cycle detection.
    #[arg(long, default_value_t = false)]
    detect_cycles: bool,

    /// Max children per node in dense shape. Node i depends on all j where
    /// i < j < i + dense_width. 0 means unlimited (node i depends on all j > i).
    #[arg(long, default_value_t = 0)]
    dense_width: u32,

    /// Number of independent copies of the graph. All copies share the same
    /// Seed and Buffer keys but have separate GraphNode and Buffer instances.
    #[arg(long, default_value_t = 1)]
    shadows: u32,

    /// Additional tokio worker threads beyond `available_parallelism()`.
    #[arg(long, default_value_t = 0)]
    extra_workers: u32,

    /// Override absolute tokio worker count (overrides `--extra-workers`).
    #[arg(long)]
    workers: Option<u32>,

    /// Disable tokio's per-worker LIFO slot.
    #[arg(long, default_value_t = false)]
    no_lifo: bool,
}

impl Args {
    fn total_nodes(&self) -> u32 {
        match self.shape {
            Shape::Wide | Shape::Chain | Shape::Dense => self.node_count,
            Shape::Tree => {
                if self.branching <= 1 {
                    self.depth + 1
                } else {
                    (self.branching.pow(self.depth + 1) - 1) / (self.branching - 1)
                }
            }
        }
    }
}

// --- Keys ---

/// Injected root key. Changes each iteration to trigger invalidation.
/// Shared across all shadow copies.
#[derive(Clone, Debug, Display, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Seed")]
#[pagable_typetag(DiceKeyDyn)]
struct Seed;

impl InjectedKey for Seed {
    type Value = u32;

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// Depends on Seed but always returns 0. Absorbs the change so that
/// downstream nodes see no value change. Each shadow has its own Buffer.
#[derive(Clone, Debug, Display, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Buffer({})", _0)]
#[pagable_typetag(DiceKeyDyn)]
struct Buffer(u32);

#[async_trait]
impl Key for Buffer {
    type Value = u32;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> u32 {
        // Read Seed to create the dependency, but ignore its value.
        let _seed = ctx.compute(&Seed).await.unwrap();
        0
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

/// A node in the stress-test graph. Dependencies are determined by the
/// configured shape (read from the global ARGS). The shadow field identifies
/// which copy of the graph this node belongs to.
#[derive(Clone, Debug, Display, Dupe, Eq, Hash, PartialEq, Allocative, Pagable)]
#[display("Node({}, shadow={})", _0, _1)]
#[pagable_typetag(DiceKeyDyn)]
struct GraphNode(u32, u32);

#[async_trait]
impl Key for GraphNode {
    type Value = u32;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> u32 {
        let args = ARGS.get().unwrap();
        let shadow = self.1;
        match args.shape {
            Shape::Wide => {
                if self.0 == 0 {
                    // Root: fan out to all other nodes.
                    ctx.compute_join(1..args.node_count, async |ctx, i| {
                        drop(ctx.compute(&GraphNode(i, shadow)).await);
                    })
                    .await;
                } else {
                    // Leaf: depend on Buffer.
                    drop(ctx.compute(&Buffer(shadow)).await);
                }
            }
            Shape::Chain => {
                if self.0 == args.node_count - 1 {
                    // End of chain: depend on Buffer.
                    drop(ctx.compute(&Buffer(shadow)).await);
                } else {
                    // Link: depend on next node.
                    drop(ctx.compute(&GraphNode(self.0 + 1, shadow)).await);
                }
            }
            Shape::Tree => {
                let total = args.total_nodes();
                let first_child = self.0 * args.branching + 1;
                if first_child < total {
                    // Interior node: depend on children.
                    let last_child = (first_child + args.branching).min(total);
                    ctx.compute_join(first_child..last_child, async |ctx, i| {
                        drop(ctx.compute(&GraphNode(i, shadow)).await);
                    })
                    .await;
                } else {
                    // Leaf: depend on Buffer.
                    drop(ctx.compute(&Buffer(shadow)).await);
                }
            }
            Shape::Dense => {
                let end = if args.dense_width == 0 {
                    args.node_count
                } else {
                    (self.0 + 1 + args.dense_width).min(args.node_count)
                };
                if self.0 == args.node_count - 1 {
                    // Last node: depend on Buffer.
                    drop(ctx.compute(&Buffer(shadow)).await);
                } else {
                    // Depend on nodes in range (self.0+1)..end.
                    // Last dep in range also transitively reaches Buffer.
                    ctx.compute_join(self.0 + 1..end, async |ctx, i| {
                        drop(ctx.compute(&GraphNode(i, shadow)).await);
                    })
                    .await;
                }
            }
        }
        self.0
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }

    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

fn print_stats(label: &str, durations: &[Duration]) {
    if durations.is_empty() {
        return;
    }
    let min = durations.iter().min().unwrap();
    let max = durations.iter().max().unwrap();
    let sum: Duration = durations.iter().sum();
    let avg = sum / durations.len() as u32;
    eprintln!(
        "{}: avg={:.3}ms  min={:.3}ms  max={:.3}ms",
        label,
        avg.as_secs_f64() * 1000.0,
        min.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0,
    );
}

fn main() {
    let args = Args::parse();
    ARGS.set(args).unwrap();

    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    let default_workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let worker_count = args
        .workers
        .map(|w| w as usize)
        .unwrap_or(default_workers + args.extra_workers as usize);
    builder.worker_threads(worker_count);
    if args.no_lifo {
        builder.disable_lifo_slot();
    }
    let rt = builder.build().unwrap();
    eprintln!(
        "Runtime: workers={}, lifo={}",
        worker_count,
        if args.no_lifo { "disabled" } else { "enabled" },
    );
    rt.block_on(async_main());
}

async fn async_main() {
    let args = ARGS.get().unwrap();

    let total = args.total_nodes();
    eprintln!("Config: {:?}", args);
    eprintln!(
        "Shape: {:?}, graph nodes per shadow: {}, shadows: {}, total DICE keys: {} ({} graph + {} Buffer + 1 Seed)",
        args.shape,
        total,
        args.shadows,
        total * args.shadows + args.shadows + 1,
        total * args.shadows,
        args.shadows,
    );

    let dice = Dice::builder().build(if args.detect_cycles {
        DetectCycles::Enabled
    } else {
        DetectCycles::Disabled
    });
    let root = dice.new_root().await;

    // Initial computation: populate the entire graph for all shadows in parallel.
    let start = Instant::now();
    {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, 0)]).unwrap();
        let ctx = updater.commit().await;
        ctx.ctx()
            .compute_join(0..args.shadows, async |ctx, s| {
                drop(ctx.compute(&GraphNode(0, s)).await);
            })
            .await;
    }
    eprintln!(
        "Initial computation: {:.3}s\n",
        start.elapsed().as_secs_f64()
    );

    // Invalidation iterations: change Seed, recompute all shadow roots, measure cost.
    let mut times = Vec::with_capacity(args.iterations as usize);
    let mut commit_times = Vec::with_capacity(args.iterations as usize);
    let mut compute_times = Vec::with_capacity(args.iterations as usize);

    for i in 1..=args.iterations {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, i)]).unwrap();

        let t0 = Instant::now();
        let ctx = updater.commit().await;
        let commit_dur = t0.elapsed();

        let t1 = Instant::now();
        ctx.ctx()
            .compute_join(0..args.shadows, async |ctx, s| {
                drop(ctx.compute(&GraphNode(0, s)).await);
            })
            .await;
        let compute_dur = t1.elapsed();

        let t2 = Instant::now();
        drop(ctx);
        dice.wait_for_idle().await;
        let wait_dur = t2.elapsed();

        let total_dur = t0.elapsed();
        times.push(total_dur);
        commit_times.push(commit_dur);
        compute_times.push(compute_dur);

        eprintln!(
            "  Iteration {:>3}: commit={:>8.3}ms  compute={:>8.3}ms  wait={:>8.3}ms total={:>8.3}ms",
            i,
            commit_dur.as_secs_f64() * 1000.0,
            compute_dur.as_secs_f64() * 1000.0,
            wait_dur.as_secs_f64() * 1000.0,
            total_dur.as_secs_f64() * 1000.0,
        );
    }

    eprintln!("\n--- Summary ({} iterations) ---", args.iterations);
    print_stats("Total (commit+compute)", &times);
    print_stats("  Commit", &commit_times);
    print_stats("  Compute", &compute_times);
}
