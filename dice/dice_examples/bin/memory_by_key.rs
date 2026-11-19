/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Measure DICE per-key memory cost. Just `buck2 run` it and read the
//! report at the bottom. By default the output is a compact progress list
//! plus the final report; set `MEMORY_BY_KEY_VERBOSE=1` in the environment
//! to also dump the per-shadow / per-copy measurement traces underneath
//! each phase.
//!
//! What it does, in order:
//!
//! 1. **Quality checks.** Run a small throwaway compute on a fresh
//!    `Dice`, then measure (a) heap drift over an idle window — the
//!    "noise floor" — and (b) heap delta from allocating a known
//!    1 MiB `Box<[u8]>` — the "calibration factor". These, plus the
//!    steady-state stability of the trailing deltas, are distilled into
//!    the `GOOD` / `QUESTIONABLE` verdict the report opens with, so you
//!    can tell at a glance whether the numbers are worth trusting.
//!
//! 2. **Three measurement phases**, each on a fresh `Dice`:
//!    - `chain` — 1 dep + 1 rdep per key (`SeriesParallelDeps::One`)
//!    - `dense w=5` — 5 deps + 5 rdeps per key (`Many` variant)
//!    - `dense w=20` — 20 deps + 20 rdeps per key (`Many` variant)
//!
//!    Each phase warms up, then builds N shadows of the graph one at a
//!    time, snapshotting after each. The reported per-key cost is the
//!    **minimum of the trailing few per-shadow heap deltas**: the
//!    trailing window skips lazy first-time allocations, and since
//!    HashMap rehashes only ever inflate a delta, the minimum is the
//!    closest estimate of the rehash-free steady state.
//!
//! 3. **Per-active-tx test** (only on dense w=20): bump `Seed` and
//!    recompute all roots while holding the ctx alive. `Buffer` absorbs
//!    the change so no recomputation runs, but the `SharedCache` must
//!    hold an entry for every key visited. This isolates the per-key
//!    cost of the `SharedCache` entries an active transaction keeps
//!    alive, plus how much of that persists after the ctx is dropped.
//!
//! 4. **Suspended-cost phases** (chain, dense w=5, dense w=20): build `SHADOWS`
//!    independent copies of the graph, one at a time, each driven by its
//!    own spawned transaction whose leaf parks on a shared gate instead
//!    of returning. Once a copy has fully spun up and every one of its
//!    computations is parked on the leaf, snapshot the heap. Because
//!    nothing has finished, this captures what a *suspended* computation
//!    costs — the in-flight futures, DICE's per-task scratch, and the
//!    `SharedCache` entries the live transactions hold — none of which
//!    exist at rest. Per-key cost is the minimum of the trailing
//!    per-copy deltas, same as the at-rest phases.
//!
//! 5. **Final report** linearly fits the three at-rest phases to extract
//!    per-edge cost, fixed cost for the One and Many variants, and the
//!    `Box<SPDepsMany>` overhead, then prints the per-tx and suspended
//!    breakdowns. It shows one number per metric on its own fixed line,
//!    so two runs diff cleanly against each other.
//!
//! Methodology choices and their reasons:
//! - Single-threaded tokio runtime: removes per-worker scratch noise.
//! - `wait_for_idle()` + sleep before every snapshot: lets deferred drops
//!   settle so the heap reading is steady.
//! - The suspended phases can't use `wait_for_idle()` — their parked
//!   transactions never go idle — so they sleep to quiesce, and detect
//!   full spin-up via a counter that every `compute` bumps.
//! - jemalloc `stats.allocated` via `mallctl` (fbcode rust binaries link
//!   jemalloc): byte-granular, immune to LLVM eliding alloc+free pairs,
//!   refreshed via `epoch` advance before each read.

// fbcode rust binaries link (unprefixed) jemalloc by default; in a cargo build we have to bring
// it ourselves or the `mallctl` heap metric below won't even link. Mirrors app/buck2/bin/buck2.rs;
// the `unprefixed_malloc_on_supported_platforms` feature is what makes plain `mallctl` resolve.
#[global_allocator]
#[cfg(all(any(target_os = "linux", target_os = "macos"), not(buck_build)))]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
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

#[derive(Copy, Clone)]
struct Snapshot {
    heap: isize,
}

impl Snapshot {
    fn take() -> Self {
        Self {
            heap: read_heap_allocated_bytes(),
        }
    }
}

/// Bytes currently allocated by the application according to the heap
/// allocator. fbcode rust binaries link jemalloc by default (the
/// `_je_*` symbols are present in this binary), so we use jemalloc's
/// `mallctl("stats.allocated", ...)` API.
///
/// jemalloc maintains stats lazily; we must advance the epoch before
/// each read, otherwise we get a stale value.
///
/// Byte-granular, immune to LLVM elision (asks the allocator directly),
/// and counts everything that goes through `malloc` (which is everything
/// dice does).
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_heap_allocated_bytes() -> isize {
    let mut epoch: u64 = 1;
    let mut sz_epoch: usize = std::mem::size_of::<u64>();
    // SAFETY: name is a valid nul-terminated C string. `oldp` and
    // `oldlenp` point to a `u64` and its length; `newp` and `newlen`
    // similarly. Per jemalloc API: passing both old and new advances
    // the epoch and returns the previous value.
    unsafe {
        mallctl(
            c"epoch".as_ptr(),
            (&raw mut epoch).cast(),
            &raw mut sz_epoch,
            (&raw mut epoch).cast(),
            sz_epoch,
        );
    }

    let mut allocated: usize = 0;
    let mut sz_allocated: usize = std::mem::size_of::<usize>();
    // SAFETY: name is a valid nul-terminated C string. `oldp` points
    // to a `usize` and `oldlenp` to its length; `newp`/`newlen` are
    // null/0 since we're reading.
    let rc = unsafe {
        mallctl(
            c"stats.allocated".as_ptr(),
            (&raw mut allocated).cast(),
            &raw mut sz_allocated,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        // mallctl failed (e.g., jemalloc compiled without --enable-stats).
        // Return -1 as a sentinel; the user will see the value sticking
        // and know the metric is broken.
        return -1;
    }
    allocated as isize
}

/// There is no jemalloc on Windows. The Buck target is Linux-only, but cargo
/// has no way to leave a `[[bin]]` out on one OS, so the workspace builds this
/// binary there too; the metric reads as the broken-metric sentinel.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_heap_allocated_bytes() -> isize {
    -1
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
unsafe extern "C" {
    // tikv_jemallocator uses a `_rjem_` prefix for its functions if prefixing is enabled:
    // <https://docs.rs/crate/tikv-jemalloc-sys/0.7.1+5.3.1-0-g81034ce1f1373e37dc865038e1bc8eeecf559ce8/source/src/lib.rs#143>
    // and the prefixing is force-enabled on Apple:
    // <https://docs.rs/crate/tikv-jemalloc-sys/0.7.1+5.3.1-0-g81034ce1f1373e37dc865038e1bc8eeecf559ce8/source/src/env.rs#24>
    // thus we have to prefix it. this would be avoidable if they exposed this function as API :)
    #[cfg_attr(target_os = "macos", link_name = "_rjem_mallctl")]
    fn mallctl(
        name: *const std::ffi::c_char,
        oldp: *mut std::ffi::c_void,
        oldlenp: *mut usize,
        newp: *mut std::ffi::c_void,
        newlen: usize,
    ) -> std::ffi::c_int;
}

fn fmt_bytes(b: isize) -> String {
    let abs = b.unsigned_abs() as f64;
    let sign = if b < 0 { "-" } else { "" };
    if abs >= 1024.0 * 1024.0 {
        format!("{}{:.2} MiB", sign, abs / (1024.0 * 1024.0))
    } else if abs >= 1024.0 {
        format!("{}{:.2} KiB", sign, abs / 1024.0)
    } else {
        format!("{}{} B", sign, abs as i64)
    }
}

/// Whether to print the per-shadow / per-copy measurement traces. Off by
/// default to keep the output to the progress list and the final report;
/// `MEMORY_BY_KEY_VERBOSE=1` turns the traces back on for debugging a
/// surprising number. Read once and cached.
fn verbose() -> bool {
    static VERBOSE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *VERBOSE.get_or_init(|| std::env::var_os("MEMORY_BY_KEY_VERBOSE").is_some())
}

// --- Configuration -----------------------------------------------------------
//
// All hardcoded — there are no command-line flags. The tool runs three
// phases on fresh `Dice` instances and prints one unified report.

const NODE_COUNT: u32 = 1000;
const SHADOWS: u32 = 10;
const SLOPE_WINDOW: u32 = 5;
const WARMUP_KEYS: u32 = 200;
const QUIESCE_MS: u64 = 100;
const WARMUP_SHADOW_ID: u32 = u32::MAX;

#[derive(Clone, Copy, Debug)]
enum Shape {
    /// Linear chain: node i depends on node i+1; last depends on Buffer.
    /// Each node has 1 dep + 1 rdep ≈ 2 edges; uses
    /// `SeriesParallelDeps::One` variant (no Box overhead).
    Chain,
    /// Dense DAG: node i depends on nodes (i+1)..(i+1+dense_width).
    /// Each node has ≈ dense_width deps + dense_width rdeps; uses
    /// `SeriesParallelDeps::Many` variant (`Box<SPDepsMany>` overhead).
    Dense { dense_width: u32 },
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Chain => "chain    ",
            Shape::Dense { dense_width: 5 } => "dense w=5",
            Shape::Dense { dense_width: 20 } => "dense w=20",
            Shape::Dense { dense_width: 64 } => "dense w=64",
            Shape::Dense { .. } => "dense    ",
        }
    }

    /// Total directed edges in the populated graph (one shadow), counting
    /// each parent→child edge once. The same number is also rdeps.
    fn edges_per_shadow(self) -> u64 {
        let n = NODE_COUNT as u64;
        match self {
            Shape::Chain => n, // n-1 chain links + last→Buffer
            Shape::Dense { dense_width } => {
                let w = dense_width as u64;
                let mut total = 0u64;
                for i in 0..(n - 1) {
                    let end = (i + 1 + w).min(n);
                    total += end - (i + 1);
                }
                total + 1 // + last→Buffer
            }
        }
    }
}

/// The current shape, swapped between phases. Read from inside
/// `GraphNode::compute`.
static CURRENT_SHAPE: std::sync::Mutex<Shape> = std::sync::Mutex::new(Shape::Chain);

fn set_shape(s: Shape) {
    *CURRENT_SHAPE.lock().unwrap() = s;
}

fn get_shape() -> Shape {
    *CURRENT_SHAPE.lock().unwrap()
}

/// Gate the blocking leaf parks on during the suspended-cost phase.
/// In suspend mode every `Buffer` leaf acquires (and forgets) a permit
/// before returning; the harness adds one permit per leaf to release them
/// all once it has measured the fully-parked graph. Starts empty, so the
/// leaves block until released.
static LEAF_GATE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(0);

/// When true, `Buffer::compute` parks on `LEAF_GATE` instead of returning
/// immediately. Set only for the duration of the suspended-cost phase.
static SUSPEND_MODE: AtomicBool = AtomicBool::new(false);

/// Count of `Key::compute` bodies that have begun executing. The
/// suspended-cost harness resets this to 0 and waits for it to reach a
/// known total to detect when every computation in a graph has spun up
/// and parked on the leaf. Ignored by the other phases.
static ENTERED: AtomicU64 = AtomicU64::new(0);

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
        ENTERED.fetch_add(1, Ordering::Relaxed);
        let _seed = ctx.compute(&Seed).await.unwrap();
        if SUSPEND_MODE.load(Ordering::Relaxed) {
            // The suspended-cost phase makes this the single leaf the whole
            // graph blocks on: park here until the harness has measured the
            // fully-parked graph and releases the gate. `forget()` consumes
            // the permit so the gate returns to zero for the next phase.
            LEAF_GATE
                .acquire()
                .await
                .expect("leaf gate is never closed")
                .forget();
        }
        0
    }
    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|x, y| x == y)
    }
    fn value_serialize() -> impl dice::ValueSerialize<Value = Self::Value> {
        NoValueSerialize::<Self::Value>::new()
    }
}

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
        ENTERED.fetch_add(1, Ordering::Relaxed);
        let shadow = self.1;
        match get_shape() {
            Shape::Chain => {
                if self.0 == NODE_COUNT - 1 {
                    drop(ctx.compute(&Buffer(shadow)).await);
                } else {
                    drop(ctx.compute(&GraphNode(self.0 + 1, shadow)).await);
                }
            }
            Shape::Dense { dense_width } => {
                if self.0 == NODE_COUNT - 1 {
                    drop(ctx.compute(&Buffer(shadow)).await);
                } else {
                    let end = (self.0 + 1 + dense_width).min(NODE_COUNT);
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

/// Wait for dice to be fully idle, sleep for the configured quiesce
/// window, and then take a snapshot.
async fn quiesce_and_snap(dice: &Arc<Dice>) -> Snapshot {
    dice.wait_for_idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(QUIESCE_MS)).await;
    Snapshot::take()
}

/// Quiesce by sleeping only, then snapshot. Used while computations are
/// deliberately suspended: `wait_for_idle()` would never return, since the
/// parked transactions stay active until the leaf is released.
async fn sleep_and_snap() -> Snapshot {
    tokio::time::sleep(std::time::Duration::from_millis(QUIESCE_MS)).await;
    Snapshot::take()
}

/// Per-phase outputs we'll roll up into the unified summary.
#[derive(Debug, Clone)]
struct PhaseResult {
    label: &'static str,
    edges_per_shadow: u64,
    /// Minimum per-key cost over the trailing `SLOPE_WINDOW` per-shadow
    /// deltas — closest to the rehash-free baseline.
    per_key_min: f64,
    /// All trailing deltas, sorted ascending, in B/key. Used to gauge
    /// steady-state confidence (the min vs the 2nd-smallest).
    trailing_per_key: Vec<f64>,
    /// Tx overhead, only set on the phase where we run the tx test.
    tx: Option<TxResult>,
}

#[derive(Debug, Clone, Copy)]
struct TxResult {
    /// Per-key heap cost of the `SharedCache` entries an active tx keeps
    /// alive (measured while ctx is alive after compute).
    active: f64,
    /// Per-key heap that persists after the ctx drops (verified_ranges
    /// extension etc.). Should be tiny.
    persisted_after_drop: f64,
}

/// Per-key cost statistics derived from a sequence of per-shadow snapshots.
struct SlopeStats {
    /// Median per-key cost over the trailing `SLOPE_WINDOW` deltas.
    per_key_median: f64,
    /// Minimum per-key cost in the same window — closest to the rehash-free
    /// baseline, since HashMap rehashes only ever inflate a delta.
    per_key_min: f64,
    /// All trailing deltas, sorted ascending, in B/key (for diagnostics).
    trailing_per_key: Vec<f64>,
}

/// Median/min per-key cost from the trailing `SLOPE_WINDOW` per-shadow heap
/// deltas, each divided by `keys_per_shadow`. The trailing window skips the
/// early shadows (lazy first-time allocations); min vs median exposes
/// HashMap-rehash spikes, which only ever inflate a delta.
fn slope_stats(snaps: &[Snapshot], keys_per_shadow: f64) -> SlopeStats {
    let window = (SLOPE_WINDOW as usize).min(SHADOWS as usize);
    let n_snaps = snaps.len();
    let mut deltas: Vec<isize> = (n_snaps - window..n_snaps)
        .map(|i| snaps[i].heap - snaps[i - 1].heap)
        .collect();
    deltas.sort();
    SlopeStats {
        per_key_median: deltas[window / 2] as f64 / keys_per_shadow,
        per_key_min: *deltas.first().unwrap() as f64 / keys_per_shadow,
        trailing_per_key: deltas.iter().map(|&d| d as f64 / keys_per_shadow).collect(),
    }
}

/// Run a single phase end-to-end on a fresh `Dice` instance: warmup,
/// per-shadow build with median slope, optionally the per-tx test.
/// Drops dice and quiesces before returning.
async fn run_phase(shape: Shape, do_tx: bool) -> PhaseResult {
    set_shape(shape);
    let label = shape.label();
    let edges = shape.edges_per_shadow();
    let keys_per_shadow = (NODE_COUNT + 1) as f64; // graph + 1 Buffer

    if verbose() {
        eprintln!();
        eprintln!("─── Phase: {label} ({edges} edges/shadow) ───");
    }

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    // Warmup: compute a small chain of leaves through the same code
    // paths so tokio + dice internals grow before we start measuring.
    {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, 0u32)]).unwrap();
        let ctx = updater.commit().await;
        let n = WARMUP_KEYS.min(NODE_COUNT);
        for i in 0..n {
            drop(ctx.compute(&GraphNode(i, WARMUP_SHADOW_ID)).await);
        }
        drop(ctx);
    }

    // Per-shadow build, snapshotting after each.
    let mut snaps: Vec<Snapshot> = Vec::with_capacity(SHADOWS as usize + 1);
    snaps.push(quiesce_and_snap(&dice).await);
    for s in 0..SHADOWS {
        // Build one shadow's root computation; dice walks the whole graph.
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, 0u32)]).unwrap();
        let ctx = updater.commit().await;
        drop(ctx.compute(&GraphNode(0, s)).await);
        drop(ctx);
        let snap = quiesce_and_snap(&dice).await;
        if verbose() {
            let dh = snap.heap - snaps.last().unwrap().heap;
            eprintln!(
                "  shadow {s}: Δheap={:>10}  ({:>6.1} B/key)",
                fmt_bytes(dh),
                dh as f64 / keys_per_shadow,
            );
        }
        snaps.push(snap);
    }

    let stats = slope_stats(&snaps, keys_per_shadow);
    let s_post_build = *snaps.last().unwrap();
    if verbose() {
        eprintln!(
            "  trailing per-key (sorted, B/key): {:?}  → median {:.1}, min {:.1}",
            stats
                .trailing_per_key
                .iter()
                .map(|v| format!("{v:.0}"))
                .collect::<Vec<_>>(),
            stats.per_key_median,
            stats.per_key_min,
        );
    }

    // Optional per-tx test.
    let tx = if do_tx {
        let total_keys = (NODE_COUNT as u64 + 1) * SHADOWS as u64;
        let total_keys_f = total_keys as f64;

        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, 1u32)]).unwrap();
        let ctx = updater.commit().await;
        for s in 0..SHADOWS {
            drop(ctx.compute(&GraphNode(0, s)).await);
        }
        tokio::time::sleep(std::time::Duration::from_millis(QUIESCE_MS)).await;
        let s_tx_active = Snapshot::take();
        let active = (s_tx_active.heap - s_post_build.heap) as f64 / total_keys_f;
        if verbose() {
            eprintln!(
                "  tx (ctx alive):                 {:>10}  ({:>6.1} B/key)",
                fmt_bytes(s_tx_active.heap - s_post_build.heap),
                active,
            );
        }

        drop(ctx);
        let s_post_tx = quiesce_and_snap(&dice).await;
        let persisted = (s_post_tx.heap - s_post_build.heap) as f64 / total_keys_f;
        if verbose() {
            eprintln!(
                "  persisted after ctx drop:       {:>10}  ({:>6.1} B/key)",
                fmt_bytes(s_post_tx.heap - s_post_build.heap),
                persisted,
            );
        }

        Some(TxResult {
            active,
            persisted_after_drop: persisted,
        })
    } else {
        None
    };

    drop(dice);
    tokio::time::sleep(std::time::Duration::from_millis(QUIESCE_MS)).await;

    PhaseResult {
        label,
        edges_per_shadow: edges,
        per_key_min: stats.per_key_min,
        trailing_per_key: stats.trailing_per_key,
        tx,
    }
}

/// Per-shape output of the suspended-cost measurement.
#[derive(Debug, Clone)]
struct SuspendedResult {
    label: &'static str,
    /// Minimum per-key heap cost of a *suspended* computation over the
    /// trailing `SLOPE_WINDOW` per-copy deltas (each copy adds one
    /// fully-parked graph of `keys_per_shadow` in-flight computations) —
    /// closest to the rehash-free baseline, matching the at-rest phases.
    per_key_min: f64,
}

/// Measure the per-key heap cost of a *suspended* computation for one
/// shape. Builds `SHADOWS` independent copies of the graph, one at a time,
/// each driven by its own spawned transaction whose leaf parks on
/// `LEAF_GATE`. After each copy has fully spun up and parked, snapshots the
/// heap while everything is idle-but-suspended — capturing the in-flight
/// futures, DICE's per-task scratch, and the `SharedCache` entries the live
/// transactions hold, none of which exist at rest. The per-copy heap delta
/// divided by `keys_per_shadow` is the suspended per-key cost; we report the
/// median and min of the trailing window, like the at-rest phases.
async fn run_suspended_phase(shape: Shape) -> SuspendedResult {
    set_shape(shape);
    let label = shape.label();
    let keys_per_shadow = (NODE_COUNT + 1) as f64; // graph nodes + 1 leaf

    if verbose() {
        eprintln!();
        eprintln!("─── Suspended phase: {label} ───");
    }

    let dice = Dice::builder().build(DetectCycles::Disabled);
    let root = dice.new_root().await;

    // Warmup through the same code paths (not in suspend mode) so tokio +
    // dice internals grow before we start measuring.
    {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, 0u32)]).unwrap();
        let ctx = updater.commit().await;
        let n = WARMUP_KEYS.min(NODE_COUNT);
        for i in 0..n {
            drop(ctx.compute(&GraphNode(i, WARMUP_SHADOW_ID)).await);
        }
        drop(ctx);
    }

    // Baseline: warmed, idle dice. Taken before suspend mode is armed, so
    // `wait_for_idle()` is safe here.
    let baseline = quiesce_and_snap(&dice).await;
    SUSPEND_MODE.store(true, Ordering::Relaxed);
    ENTERED.store(0, Ordering::Relaxed);

    // Spawn the graph copies one at a time, snapshotting once each has fully
    // parked. Each copy is its own transaction driven by its own task: the
    // task holds the root compute future alive, which keeps the whole
    // cascade of DICE tasks alive (a dropped compute future would cancel
    // them). Distinct `shadow` ids make every copy a fresh set of keys.
    let mut handles: Vec<tokio::task::JoinHandle<()>> = Vec::with_capacity(SHADOWS as usize);
    let mut snaps: Vec<Snapshot> = Vec::with_capacity(SHADOWS as usize + 1);
    snaps.push(baseline);

    for s in 0..SHADOWS {
        let mut updater = dice.updater(root);
        updater.changed_to(vec![(Seed, 0u32)]).unwrap();
        let ctx = updater.commit().await;
        handles.push(tokio::spawn(async move {
            let ctx = ctx;
            drop(ctx.compute(&GraphNode(0, s)).await);
        }));

        // Wait until this copy's computations have all begun (and thus are
        // parked, or about to park, on the leaf). Each copy adds NODE_COUNT
        // graph nodes + 1 leaf to ENTERED. No other copy is spinning up yet,
        // so once the target is reached ENTERED is stable and the snapshot
        // is of a fully-quiescent suspended graph.
        let target = (s as u64 + 1) * (NODE_COUNT as u64 + 1);
        while ENTERED.load(Ordering::Relaxed) < target {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }

        let snap = sleep_and_snap().await;
        if verbose() {
            let dh = snap.heap - snaps.last().unwrap().heap;
            eprintln!(
                "  copy {s}: Δheap={:>10}  ({:>6.1} B/key suspended)",
                fmt_bytes(dh),
                dh as f64 / keys_per_shadow,
            );
        }
        snaps.push(snap);
    }

    // Release every parked leaf and let all copies run to completion.
    LEAF_GATE.add_permits(SHADOWS as usize);
    for h in handles {
        h.await.unwrap();
    }
    SUSPEND_MODE.store(false, Ordering::Relaxed);

    let stats = slope_stats(&snaps, keys_per_shadow);
    if verbose() {
        eprintln!(
            "  trailing per-key (sorted, B/key): {:?}  → median {:.1}, min {:.1}",
            stats
                .trailing_per_key
                .iter()
                .map(|v| format!("{v:.0}"))
                .collect::<Vec<_>>(),
            stats.per_key_median,
            stats.per_key_min,
        );
    }

    drop(dice);
    tokio::time::sleep(std::time::Duration::from_millis(QUIESCE_MS)).await;

    SuspendedResult {
        label,
        per_key_min: stats.per_key_min,
    }
}

fn main() {
    // Single-threaded runtime: removes per-worker scratch space and any
    // tokio thread-local state from the measurement so deltas are closer
    // to "what dice itself allocates".
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async_main());
}

async fn async_main() {
    // Off-verbose, the only output until the report is a single growing
    // progress line so the run doesn't look hung; on-verbose, the phase
    // runners print their own traces instead.
    let progress = !verbose();

    eprintln!(
        "DICE memory profile  (NODE_COUNT={NODE_COUNT}, SHADOWS={SHADOWS}; jemalloc stats.allocated, single-thread tokio)"
    );
    if verbose() {
        eprintln!(
            "  config: SLOPE_WINDOW={SLOPE_WINDOW}, WARMUP_KEYS={WARMUP_KEYS}, QUIESCE_MS={QUIESCE_MS}"
        );
    } else {
        eprintln!("  (set MEMORY_BY_KEY_VERBOSE=1 for per-shadow traces and methodology)");
    }
    if progress {
        eprint!("\nMeasuring:");
    }

    // ── Calibration & noise floor on a fresh dice (smallest setup) ──
    let dice_cal = Dice::builder().build(DetectCycles::Disabled);
    let dice_cal_root = dice_cal.new_root().await;
    set_shape(Shape::Chain);
    {
        // Tiny warmup so the first read isn't on a cold runtime.
        let mut updater = dice_cal.updater(dice_cal_root);
        updater.changed_to(vec![(Seed, 0u32)]).unwrap();
        let ctx = updater.commit().await;
        for i in 0..50 {
            drop(ctx.compute(&GraphNode(i, WARMUP_SHADOW_ID)).await);
        }
        drop(ctx);
    }
    let s1 = quiesce_and_snap(&dice_cal).await;
    tokio::time::sleep(std::time::Duration::from_millis(QUIESCE_MS * 2)).await;
    let s2 = Snapshot::take();
    let noise_heap = s2.heap - s1.heap;

    let probe: Box<[u8]> = vec![0u8; 1024 * 1024].into_boxed_slice();
    std::hint::black_box(&probe);
    let s_probe = Snapshot::take();
    let cal_factor = (s_probe.heap - s2.heap) as f64 / (1024.0 * 1024.0);
    drop(probe);
    drop(dice_cal);
    if progress {
        eprint!(" quality");
    }

    // ── Three measurement phases on fresh Dice instances ──
    let chain = run_phase(Shape::Chain, false).await;
    if progress {
        eprint!(", chain");
    }
    let dense5 = run_phase(Shape::Dense { dense_width: 5 }, false).await;
    if progress {
        eprint!(", dense w=5");
    }
    let dense20 = run_phase(Shape::Dense { dense_width: 20 }, true).await;
    if progress {
        eprint!(", dense w=20");
    }
    // w=64 crosses a behavior change in how the parallel branches get awaited:
    // futures' `join_all` switches to `FuturesOrdered` above 30 futures, so the
    // per-branch suspended cost includes the collection's bookkeeping. Its at-rest
    // value is honest — `run_phase` snapshots via `quiesce_and_snap` (wait_for_idle
    // + sleep) after each shadow's compute finishes and its ctx is dropped, so
    // any FuturesOrdered state is gone before the reading. w=64 is still excluded
    // from the per-edge linear fit because the fit runs over only two dense
    // widths and we want the coefficient anchored in the sub-30 regime where the
    // awaiter behavior matches w=5 and w=20.
    let dense64 = run_phase(Shape::Dense { dense_width: 64 }, false).await;
    if progress {
        eprint!(", dense w=64");
    }

    // ── Suspended-cost phases on fresh Dice instances ──
    if progress {
        eprint!("  |  suspended:");
    }
    let chain_susp = run_suspended_phase(Shape::Chain).await;
    if progress {
        eprint!(" chain");
    }
    let dense5_susp = run_suspended_phase(Shape::Dense { dense_width: 5 }).await;
    if progress {
        eprint!(", dense w=5");
    }
    let dense20_susp = run_suspended_phase(Shape::Dense { dense_width: 20 }).await;
    if progress {
        eprint!(", dense w=20");
    }
    let dense64_susp = run_suspended_phase(Shape::Dense { dense_width: 64 }).await;
    if progress {
        eprintln!(", dense w=64");
    }

    // ── Linear regression of per-key cost vs edges ──
    // We use all three (chain, dense5, dense20). chain uses the
    // `SeriesParallelDeps::One` variant (no Box), the dense phases use
    // `Many` (with Box<SPDepsMany> overhead). Chain's intercept is
    // therefore lower than dense's.
    //
    // Use `per_key_min` (minimum trailing delta), not median: HashMap
    // rehashes only ever ADD to a delta, never subtract, so the minimum
    // is the closest approximation of the rehash-free steady-state cost.
    // Median gets dragged up if any rehash spike falls in the trailing
    // window, which happens around every 4× growth in cumulative keys.
    let per_edge = (dense20.per_key_min - dense5.per_key_min)
        / (dense20.edges_per_shadow as f64 - dense5.edges_per_shadow as f64)
        * NODE_COUNT as f64; // edges_per_shadow is total edges; per-key edges ≈ that / NODE_COUNT
    let dense_intercept =
        dense5.per_key_min - per_edge * (dense5.edges_per_shadow as f64 / NODE_COUNT as f64);
    let chain_intercept =
        chain.per_key_min - per_edge * (chain.edges_per_shadow as f64 / NODE_COUNT as f64);
    let many_box_overhead = dense_intercept - chain_intercept;

    // ── Signal quality ──
    // Three independent ways the numbers can be untrustworthy, each turned
    // into a pass/fail with a threshold so the verdict is a judgment, not a
    // pile of raw stats the reader has to interpret:
    //  - calibration: is the absolute byte scale right (heap vs a known Box)?
    //  - noise: is the idle heap drift small next to one shadow's signal? A
    //    per-shadow step adds ~per_key_min × keys_per_shadow bytes, so noise
    //    as a fraction of that is how much it can corrupt a single delta.
    //  - steady state: is the smallest trailing delta a stable floor, or a
    //    one-off dip below the next-smallest?
    let keys_per_shadow = NODE_COUNT as f64 + 1.0;
    let per_shadow_step = dense20.per_key_min * keys_per_shadow;
    let metric_ok = cal_factor > 0.0 && per_shadow_step > 0.0;
    let noise_ratio = noise_heap as f64 / per_shadow_step;
    // Second-smallest minus smallest of the trailing window (dense w=20),
    // relative to the min: if they're close the floor is solid; if the
    // second-smallest is well above, the min may be a noise dip.
    let trailing = &dense20.trailing_per_key; // already sorted ascending
    let min_to_2nd =
        trailing.get(1).copied().unwrap_or(0.0) - trailing.first().copied().unwrap_or(0.0);
    let steady_ratio = if dense20.per_key_min > 0.0 {
        min_to_2nd / dense20.per_key_min
    } else {
        f64::INFINITY
    };

    let cal_ok = (0.95..=1.05).contains(&cal_factor);
    let mut issues: Vec<String> = Vec::new();
    if !metric_ok {
        issues.push("heap metric returned no data — was this built with jemalloc?".to_owned());
    } else {
        if !cal_ok {
            issues.push(format!(
                "calibration {cal_factor:.3}× is off by >5%; absolute scale is biased"
            ));
        }
        if noise_ratio > 0.05 {
            issues.push(format!(
                "noise floor is {:.0}% of a per-shadow step; per-key numbers are rough",
                noise_ratio * 100.0
            ));
        }
        if steady_ratio > 0.10 {
            issues.push(format!(
                "2nd-smallest trailing delta is {:.0}% above the min; steady state may be unsettled",
                steady_ratio * 100.0
            ));
        }
    }

    // ── Final report ──
    //
    // One number per metric, each on its own fixed-position line, so two
    // runs diff line-by-line. Diagnostics that clutter that comparison
    // (medians, rehash bias, per-shadow deltas, methodology) live in
    // verbose mode and the module docs, not here. Values are rounded to
    // whole bytes — finer precision is below the measurement noise.
    let signal = if issues.is_empty() {
        "GOOD"
    } else {
        "QUESTIONABLE"
    };
    eprintln!();
    eprintln!("════════════════════════════════════════════════════════════");
    eprintln!("  {:<40}signal: {signal}", "DICE PER-KEY MEMORY");
    eprintln!("════════════════════════════════════════════════════════════");
    for issue in &issues {
        eprintln!("  - {issue}");
    }
    if verbose() {
        eprintln!(
            "  calibration {:.3}×, noise floor {} ({:.0}% of a per-shadow step), steady state +{:.1} B/key over min",
            cal_factor,
            fmt_bytes(noise_heap),
            noise_ratio * 100.0,
            min_to_2nd,
        );
    }
    if !metric_ok {
        return;
    }

    eprintln!();
    eprintln!("  At-rest heap (B/key)");
    for p in [&chain, &dense5, &dense20, &dense64] {
        eprintln!("    {:<24}{:>6.0}", p.label.trim_end(), p.per_key_min);
    }

    eprintln!();
    eprintln!("  Decomposition (B)");
    eprintln!("    {:<24}{:>6.1}", "per-edge", per_edge);
    eprintln!("    {:<24}{:>6.0}", "fixed (One variant)", chain_intercept);
    eprintln!("    {:<24}{:>6.0}", "fixed (Many variant)", dense_intercept);
    eprintln!("    {:<24}{:>6.0}", "Many vs One (Box)", many_box_overhead);

    if let Some(tx) = dense20.tx {
        eprintln!();
        eprintln!("  Per-active-tx (B/key)");
        eprintln!("    {:<24}{:>6.0}", "while ctx alive", tx.active);
        eprintln!(
            "    {:<24}{:>6.0}",
            "persists after drop", tx.persisted_after_drop
        );
    }

    eprintln!();
    eprintln!("  Suspended in-flight (B/key)");
    for (rest, susp) in [
        (&chain, &chain_susp),
        (&dense5, &dense5_susp),
        (&dense20, &dense20_susp),
        (&dense64, &dense64_susp),
    ] {
        eprintln!(
            "    {:<24}{:>6.0}",
            susp.label.trim_end(),
            susp.per_key_min - rest.per_key_min,
        );
    }
}
