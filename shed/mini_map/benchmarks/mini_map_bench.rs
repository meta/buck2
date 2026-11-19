/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Microbenchmark of the per-operation costs of this crate's maps, with `std::collections::HashMap`
//! on the same hasher as the baseline.
//!
//! Nothing runs this automatically. It is for comparing a change against its parent by hand: run
//! both in opt mode on the same machine and diff the tables.
//!
//! ```text
//! buck2 run @fbcode//mode/opt fbcode//buck2/shed/mini_map:mini_map_bench -- [FILTER]
//! ```
//!
//! `FILTER` keeps the rows whose label contains it. Each number is the fastest of several timed
//! loops, so it is the cost with the noise taken out rather than an average that includes it.
//! Lookups are nanoseconds per call; construction and iteration are nanoseconds per entry, and
//! construction includes dropping the result.

use std::collections::HashMap;
use std::hash::Hash;
use std::hint::black_box;
use std::time::Instant;

use mini_map::FastHasherBuilder;
use mini_map::Hashed;
use mini_map::MiniFixedInsertionMap;
use mini_map::MiniFixedSortedMap;
use mini_map::MiniMutInsertionMap;

/// Map sizes to measure: inside the regime where lookups scan the hashes, around its edge, and
/// well past it.
const SIZES: &[usize] = &[4, 16, 64, 1_000, 100_000];

/// Timed loops per row; the fastest is reported.
const REPEATS: usize = 5;

/// Operations per timed loop, give or take.
const OPS_PER_LOOP: usize = 1 << 20;

/// Step between successive probes. Coprime with every size in `SIZES`, so probing visits every
/// entry of a map but not in storage order.
const STRIDE: usize = 7919;

/// A key type to measure with, and how to make the `i`th distinct one.
trait Key: Hash + Ord + Clone {
    const NAME: &'static str;
    fn nth(i: usize) -> Self;
}

impl Key for u64 {
    const NAME: &'static str = "u64";
    fn nth(i: usize) -> Self {
        i as u64 * 7919 + 13
    }
}

impl Key for String {
    const NAME: &'static str = "String";
    fn nth(i: usize) -> Self {
        format!("key_{i:08}")
    }
}

struct Bench {
    filter: Option<String>,
}

impl Bench {
    /// Time `op` in loops of about `OPS_PER_LOOP` operations, counting each call as `per`
    /// operations, and print the fastest loop's cost per operation.
    fn row<R>(&self, label: &str, per: usize, mut op: impl FnMut() -> R) {
        if let Some(filter) = &self.filter {
            if !label.contains(filter.as_str()) {
                return;
            }
        }
        let iters = (OPS_PER_LOOP / per).max(1);
        let mut best = f64::INFINITY;
        for _ in 0..REPEATS {
            let start = Instant::now();
            for _ in 0..iters {
                black_box(op());
            }
            let ns = start.elapsed().as_nanos() as f64 / (iters * per) as f64;
            best = best.min(ns);
        }
        println!("{label:<68}{best:9.2} ns");
    }
}

fn row_label<K: Key>(n: usize, map: &str, op: &str) -> String {
    format!("{:<7}{n:>8}  {map:<24}{op}", K::NAME)
}

/// `keys` hashed, in the order probing will visit them.
fn scramble<K: Key>(keys: &[K]) -> Vec<Hashed<K>> {
    let n = keys.len();
    let mut i = 0;
    (0..n)
        .map(|_| {
            i = (i + STRIDE) % n;
            Hashed::new(keys[i].clone())
        })
        .collect()
}

/// A row that applies `op` to the probes in turn, round and round.
fn lookup<K, R>(
    bench: &Bench,
    label: &str,
    probes: &[Hashed<K>],
    mut op: impl FnMut(&Hashed<K>) -> R,
) {
    let mut next = 0;
    bench.row(label, 1, || {
        let probe = &probes[next];
        next += 1;
        if next == probes.len() {
            next = 0;
        }
        op(probe)
    });
}

fn lookups<K: Key>(bench: &Bench, n: usize) {
    let keys: Vec<K> = (0..n).map(K::nth).collect();
    let absent: Vec<K> = (n..2 * n).map(K::nth).collect();
    let entries = || keys.iter().cloned().zip(0u64..);

    let mutable: MiniMutInsertionMap<K, u64> = entries().collect();
    let frozen: MiniFixedInsertionMap<K, u64> = entries().collect();
    let fixed: MiniFixedSortedMap<K, u64> = entries().collect();
    let std: HashMap<K, u64, FastHasherBuilder> = entries().collect();

    let hits = scramble(&keys);
    let misses = scramble(&absent);
    let label = |map: &str, op: &str| row_label::<K>(n, map, op);

    let map = "MiniMutInsertionMap";
    lookup(bench, &label(map, "get, hit"), &hits, |k| {
        mutable.get(k.key())
    });
    lookup(bench, &label(map, "get, miss"), &misses, |k| {
        mutable.get(k.key())
    });
    lookup(bench, &label(map, "get_hashed, hit"), &hits, |k| {
        mutable.get_hashed(k.as_ref())
    });
    lookup(bench, &label(map, "get_index_of, hit"), &hits, |k| {
        mutable.get_index_of(k.key())
    });
    bench.row(&label(map, "iter, summing values"), n, || {
        mutable.iter().map(|(_, v)| *v).sum::<u64>()
    });

    let map = "MiniFixedInsertionMap";
    lookup(bench, &label(map, "get, hit"), &hits, |k| {
        frozen.get(k.key())
    });
    lookup(bench, &label(map, "get, miss"), &misses, |k| {
        frozen.get(k.key())
    });
    lookup(bench, &label(map, "get_hashed, hit"), &hits, |k| {
        frozen.get_hashed(k.as_ref())
    });
    bench.row(&label(map, "iter, summing values"), n, || {
        frozen.iter().map(|(_, v)| *v).sum::<u64>()
    });

    let map = "MiniFixedSortedMap";
    lookup(bench, &label(map, "get, hit"), &hits, |k| {
        fixed.get(k.key())
    });
    lookup(bench, &label(map, "get, miss"), &misses, |k| {
        fixed.get(k.key())
    });
    lookup(bench, &label(map, "get_hashed, hit"), &hits, |k| {
        fixed.get_hashed(k.as_ref())
    });
    bench.row(&label(map, "iter, summing values"), n, || {
        fixed.iter().map(|(_, v)| *v).sum::<u64>()
    });

    let map = "HashMap<FastHasher>";
    lookup(bench, &label(map, "get, hit"), &hits, |k| std.get(k.key()));
    lookup(bench, &label(map, "get, miss"), &misses, |k| {
        std.get(k.key())
    });
    bench.row(&label(map, "iter, summing values"), n, || {
        std.values().copied().sum::<u64>()
    });
}

fn construction<K: Key>(bench: &Bench, n: usize) {
    let keys: Vec<K> = (0..n).map(K::nth).collect();
    let entries = || keys.iter().cloned().zip(0u64..);
    let hashed: Vec<(Hashed<K>, u64)> = entries().map(|(k, v)| (Hashed::new(k), v)).collect();
    let hashed_entries = || hashed.iter().cloned();
    // The same entries in probing order, which is as far from sorted as the sorted map gets.
    let shuffled: Vec<(Hashed<K>, u64)> = scramble(&keys).into_iter().zip(0u64..).collect();
    let shuffled_entries = || shuffled.iter().cloned();
    let label = |map: &str, op: &str| row_label::<K>(n, map, op);

    let map = "MiniMutInsertionMap";
    bench.row(&label(map, "collect"), n, || {
        entries().collect::<MiniMutInsertionMap<K, u64>>()
    });
    bench.row(&label(map, "insert into new()"), n, || {
        let mut m = MiniMutInsertionMap::new();
        for (k, v) in entries() {
            m.insert(k, v);
        }
        m
    });

    let map = "MiniFixedInsertionMap";
    bench.row(&label(map, "collect"), n, || {
        entries().collect::<MiniFixedInsertionMap<K, u64>>()
    });
    bench.row(&label(map, "from_hashed_unique"), n, || {
        MiniFixedInsertionMap::from_hashed_unique(hashed_entries())
    });

    let map = "MiniFixedSortedMap";
    bench.row(&label(map, "collect"), n, || {
        entries().collect::<MiniFixedSortedMap<K, u64>>()
    });
    bench.row(&label(map, "collect, pre-hashed"), n, || {
        hashed_entries().collect::<MiniFixedSortedMap<K, u64>>()
    });
    bench.row(&label(map, "collect, pre-hashed, shuffled"), n, || {
        shuffled_entries().collect::<MiniFixedSortedMap<K, u64>>()
    });

    let map = "HashMap<FastHasher>";
    bench.row(&label(map, "collect"), n, || {
        entries().collect::<HashMap<K, u64, FastHasherBuilder>>()
    });
}

fn main() {
    let bench = Bench {
        filter: std::env::args().nth(1),
    };
    println!(
        "{:<7}{:>8}  {:<24}{:<27}{:>9}",
        "key", "n", "map", "operation", "ns/op"
    );
    for &n in SIZES {
        lookups::<u64>(&bench, n);
        lookups::<String>(&bench, n);
        construction::<u64>(&bench, n);
    }
}
