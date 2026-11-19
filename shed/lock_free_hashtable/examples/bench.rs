/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Multi-threaded throughput benchmark for `ShardedLockFreeRawTable`.
//!
//! Regimes model the static-interner hot paths:
//!  - `insert_unique`: every op interns a new value (miss then insert).
//!  - `lookup_hit`: every op finds an existing value (pure reads).
//!  - `mixed`: alternating unique insert and uniform hit, like a target graph
//!    scan interning new labels while re-reading dependency labels.
//!  - `lookup_refcount`: hits on a small popular set, bumping a per-value
//!    counter the way refcounted interner handles do. This regime is the
//!    control: contention is in the value, not the table, so table layout
//!    changes should not move it.
//!
//! Prints one TSV row per measurement point:
//! `regime<TAB>shards<TAB>threads<TAB>rep1<TAB>rep2<TAB>rep3` in Mops/s.

use std::hint::black_box;
use std::io::Write;
use std::sync::Barrier;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;

use lock_free_hashtable::sharded::ShardedLockFreeRawTable;

/// Number of pre-populated entries used by lookup-containing regimes.
const PREPOP_N: u64 = 1_000_000;
/// Size of the "popular" key set for the refcount regime.
const POPULAR_N: u64 = 4096;
/// Bit set on all uniquely-inserted keys so they never collide with
/// pre-populated keys (0..PREPOP_N).
const UNIQUE_BASE: u64 = 1 << 40;
/// Repetitions per measurement point.
const REPS: usize = 3;

struct Payload {
    key: u64,
    refcount: AtomicU64,
}

type Table<const S: usize> = ShardedLockFreeRawTable<Box<Payload>, S>;

/// splitmix64 finalizer: uniform avalanche in both high (shard selection)
/// and low (bucket selection) bits.
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E3779B97F4A7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}

fn insert<const S: usize>(table: &Table<S>, key: u64) {
    let value = Box::new(Payload {
        key,
        refcount: AtomicU64::new(0),
    });
    black_box(table.insert(mix(key), value, |a, b| a.key == b.key, |v| mix(v.key)));
}

fn lookup<'a, const S: usize>(table: &'a Table<S>, key: u64) -> Option<&'a Payload> {
    table.lookup(mix(key), |p| p.key == key)
}

#[derive(Copy, Clone, PartialEq)]
enum Regime {
    InsertUnique,
    LookupHit,
    Mixed,
    LookupRefcount,
}

impl Regime {
    fn name(self) -> &'static str {
        match self {
            Regime::InsertUnique => "insert_unique",
            Regime::LookupHit => "lookup_hit",
            Regime::Mixed => "mixed",
            Regime::LookupRefcount => "lookup_refcount",
        }
    }

    fn base_ops(self) -> u64 {
        match self {
            Regime::InsertUnique => 8_000_000,
            Regime::LookupHit => 16_000_000,
            Regime::Mixed => 8_000_000,
            Regime::LookupRefcount => 16_000_000,
        }
    }

    /// Whether this regime mutates the table and therefore needs a fresh
    /// table per repetition.
    fn needs_fresh_table(self) -> bool {
        match self {
            Regime::InsertUnique | Regime::Mixed => true,
            Regime::LookupHit | Regime::LookupRefcount => false,
        }
    }
}

fn prepopulate<const S: usize>(table: &Table<S>) {
    for key in 0..PREPOP_N {
        insert(table, key);
    }
}

fn thread_body<const S: usize>(table: &Table<S>, regime: Regime, t: u64, ops: u64) {
    // Per-thread stream constant for pseudo-random key selection.
    let stream = t.wrapping_mul(0x51ed270b_9d2c5680);
    match regime {
        Regime::InsertUnique => {
            for i in 0..ops {
                insert(table, UNIQUE_BASE | (t << 32) | i);
            }
        }
        Regime::LookupHit => {
            for i in 0..ops {
                let key = mix(stream ^ i) % PREPOP_N;
                black_box(lookup(table, key));
            }
        }
        Regime::Mixed => {
            for i in 0..ops {
                if i % 2 == 0 {
                    insert(table, UNIQUE_BASE | (t << 32) | (i / 2));
                } else {
                    let key = mix(stream ^ i) % PREPOP_N;
                    black_box(lookup(table, key));
                }
            }
        }
        Regime::LookupRefcount => {
            for i in 0..ops {
                let key = mix(stream ^ i) % POPULAR_N;
                if let Some(p) = lookup(table, key) {
                    black_box(p.refcount.fetch_add(1, Ordering::Relaxed));
                }
            }
        }
    }
}

/// Run one measurement and return throughput in Mops/s.
fn run_point<const S: usize>(regime: Regime, threads: u64, shared_table: &Table<S>) -> f64 {
    let fresh_table: Table<S>;
    let table = if regime.needs_fresh_table() {
        fresh_table = ShardedLockFreeRawTable::new();
        if regime == Regime::Mixed {
            prepopulate(&fresh_table);
        }
        &fresh_table
    } else {
        shared_table
    };

    // Keep short high-thread runs long enough for the timer to be meaningful.
    let total_ops = regime.base_ops().max(threads * 250_000);
    let ops_per_thread = total_ops / threads;

    let barrier = Barrier::new(threads as usize + 1);
    let mut start_time = None;
    // The scope joins all workers before exiting, so the clock reading after
    // the scope covers exactly the released-to-all-finished interval.
    std::thread::scope(|s| {
        for t in 0..threads {
            let barrier = &barrier;
            s.spawn(move || {
                barrier.wait();
                thread_body(table, regime, t, ops_per_thread);
            });
        }
        barrier.wait();
        start_time = Some(Instant::now());
    });
    let secs = start_time.unwrap().elapsed().as_secs_f64();
    (ops_per_thread * threads) as f64 / secs / 1e6
}

fn run_suite<const S: usize>(regimes: &[Regime], thread_counts: &[u64]) {
    // Shared pre-populated table for pure-lookup regimes.
    let shared_table: Table<S> = ShardedLockFreeRawTable::new();
    prepopulate(&shared_table);

    for &regime in regimes {
        for &threads in thread_counts {
            let reps: Vec<String> = (0..REPS)
                .map(|_| format!("{:.2}", run_point::<S>(regime, threads, &shared_table)))
                .collect();
            println!("{}\t{}\t{}\t{}", regime.name(), S, threads, reps.join("\t"));
            std::io::stdout().flush().unwrap();
        }
    }
}

fn main() {
    let cores = std::thread::available_parallelism().unwrap().get() as u64;
    println!("cores\t{cores}");

    let thread_counts: Vec<u64> = [1, 4, 16, 48, 96]
        .into_iter()
        .filter(|&t| t <= cores)
        .collect();

    run_suite::<64>(
        &[
            Regime::InsertUnique,
            Regime::LookupHit,
            Regime::Mixed,
            Regime::LookupRefcount,
        ],
        &thread_counts,
    );
    // Shard-count experiment: does 4x the shards help the write-heavy regimes?
    run_suite::<256>(&[Regime::InsertUnique, Regime::Mixed], &thread_counts);
}
