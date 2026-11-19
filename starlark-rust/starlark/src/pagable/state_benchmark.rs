/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Memory + lookup-perf gates for [`StarlarkSerState`].
//!
//! Memory is measured two ways and cross-checked:
//!   1. `/proc/self/statm` RSS delta (Linux, page-granular).
//!   2. Closed-form analytic from documented hashbrown / `BTreeMap`
//!      internals — drives the assertions; OLD HashMap RSS-vs-analytic
//!      agreement validates the formula at scale.
//!
//! `allocative::size_of_unique` doesn't work here: its std `HashMap` /
//! `BTreeMap` impls don't tag the bucket array as `Unique`, so they
//! return only the struct shell (~48 B for a 20K-entry HashMap).

use std::mem;
use std::sync::Arc;
use std::time::Instant;

use allocative::Allocative;
use dashmap::DashMap;
use derive_more::Display;
use pagable::Pagable;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;
use strong_hash::StrongHash;

use crate as starlark;
use crate::pagable::heap_ref_id::HeapRefId;
use crate::pagable::starlark_serialize_context::ChunkEntry;
use crate::pagable::starlark_serialize_context::StarlarkSerState;
use crate::starlark_simple_value;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenHeap;
use crate::values::StarlarkValue;
use crate::values::layout::heap::name::FrozenHeapName;

/// Minimal `StarlarkValue` for stuffing predictable bytes into a heap.
#[derive(
    Debug,
    Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("BenchValue({}, {})", self.flag, self.count)]
struct BenchValue {
    flag: bool,
    count: usize,
}

starlark_simple_value!(BenchValue);

#[starlark_value(type = "BenchValue")]
impl<'v> StarlarkValue<'v> for BenchValue {}

#[derive(Debug, Clone, Display, Hash, StrongHash, Pagable)]
#[pagable::pagable_typetag(crate::values::UserHeapName)]
#[display("BenchHeapName({})", _0)]
struct BenchHeapName(String);

impl BenchHeapName {
    fn make(name: &str) -> FrozenHeapName {
        FrozenHeapName::User(Box::new(Self(name.to_owned())))
    }
}

/// Build `num_heaps` heaps × `values_per_heap` `BenchValue`s. Returns
/// each heap's payload pointers in serialization order (drop, then non-drop).
fn build_synthetic_heaps(
    num_heaps: usize,
    values_per_heap: usize,
) -> Vec<(OwnedFrozen<()>, HeapRefId, Vec<usize>)> {
    (0..num_heaps)
        .map(|h| {
            let heap = OwnedFrozenHeap::new();
            heap.with(|heap| {
                for v in 0..values_per_heap {
                    heap.alloc_simple(BenchValue {
                        flag: (v & 1) == 0,
                        count: v,
                    });
                }
            });
            let heap_ref = heap.seal(BenchHeapName::make(&format!("bench_{h}")));
            let heap_id = heap_ref.heap_arc().heap_ref_id().unwrap();
            let mut ptrs: Vec<usize> = Vec::new();
            ptrs.extend(
                heap_ref
                    .heap_arc()
                    .collect_drop_headers_ordered()
                    .iter()
                    .map(|hp| hp.payload_ptr().ptr as usize),
            );
            ptrs.extend(
                heap_ref
                    .heap_arc()
                    .collect_undrop_headers_ordered()
                    .iter()
                    .map(|hp| hp.payload_ptr().ptr as usize),
            );
            (heap_ref, heap_id, ptrs)
        })
        .collect()
}

/// Per-value `DashMap<usize, (HeapRefId, u32)>` — the shape that
/// shipped in `starlark_serialize_context.rs` before the chunk-index
/// rewrite. Used here as the OLD baseline in side-by-side comparisons.
fn build_per_value_dashmap(
    heaps: &[(OwnedFrozen<()>, HeapRefId, Vec<usize>)],
) -> DashMap<usize, (HeapRefId, u32)> {
    let total_values: usize = heaps.iter().map(|(_, _, ptrs)| ptrs.len()).sum();
    let map: DashMap<usize, (HeapRefId, u32)> = DashMap::with_capacity(total_values);
    for (_, heap_id, ptrs) in heaps {
        for (idx, &ptr) in ptrs.iter().enumerate() {
            map.insert(ptr, (*heap_id, idx as u32));
        }
    }
    map
}

/// Process RSS in bytes (Linux). Reads `/proc/self/statm`'s 2nd field
/// (resident pages) × 4 KB. Returns `None` on non-Linux.
fn read_rss_bytes() -> Option<usize> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let s = std::fs::read_to_string("/proc/self/statm").ok()?;
    let resident_pages: usize = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident_pages * 4096)
}

/// Hashbrown layout: capacity rounds up to the next pow2 at ≤7/8 load;
/// each slot holds (K, V) plus one control byte.
fn analytic_hashmap_bytes<K, V>(num_entries: usize) -> usize {
    let logical_cap = num_entries.div_ceil(7).saturating_mul(8); // entries / 0.875
    let cap = logical_cap.next_power_of_two().max(4);
    let slot = mem::size_of::<K>() + mem::size_of::<V>();
    let align = mem::align_of::<K>().max(mem::align_of::<V>());
    let slot_aligned = slot.div_ceil(align) * align;
    cap * slot_aligned + cap
}

/// Rust BTreeMap with B=6 packs up to 11 K/V per leaf. Approximates per
/// leaf as `11*(K+V) + 11*u16 + ~24 B header`, plus internal nodes
/// (~1/11 of leaves).
fn analytic_btreemap_bytes<K, V>(num_entries: usize) -> usize {
    let entries_per_node = 11usize;
    let leaves = num_entries.div_ceil(entries_per_node);
    let kv = mem::size_of::<K>() + mem::size_of::<V>();
    let leaf_overhead = 24usize + entries_per_node * mem::size_of::<u16>();
    let leaf_bytes = entries_per_node * kv + leaf_overhead;
    let internal_bytes = (leaves / entries_per_node).max(1) * leaf_bytes;
    leaves * leaf_bytes + internal_bytes
}

#[test]
fn test_chunk_index_beats_per_value_hashmap_memory_and_speed() {
    // ~10M values, ~10% of production scale (~570K heaps × ~196 values
    // ≈ 112M). At this size the OLD HashMap crosses ~400 MB so the
    // memory win shows up in real RSS. ~15 s on normal CI.
    const NUM_HEAPS: usize = 2_000;
    const VALUES_PER_HEAP: usize = 5_000;
    const NUM_LOOKUPS: usize = 100_000;
    let total_values = NUM_HEAPS * VALUES_PER_HEAP;

    let heaps = build_synthetic_heaps(NUM_HEAPS, VALUES_PER_HEAP);

    // NEW: chunk-index StarlarkSerState.
    let rss_before_new = read_rss_bytes();
    let state = Arc::new(StarlarkSerState::new());
    let new_build_start = Instant::now();
    for (heap_ref, _, _) in &heaps {
        state
            .ensure_chunk_index_registered(heap_ref.heap_arc())
            .expect("register chunk index");
    }
    let new_build_elapsed = new_build_start.elapsed();
    let rss_after_new = read_rss_bytes();
    let new_rss_delta = match (rss_before_new, rss_after_new) {
        (Some(b), Some(a)) => Some(a.saturating_sub(b)),
        _ => None,
    };

    let chunks_total: usize = heaps
        .iter()
        .map(|(h, _, _)| h.heap_arc().build_chunk_index().len())
        .sum();
    // `size_of::<ChunkEntry>` covers the `Box<[u32]>` header but not the
    // boxed contents — add those explicitly.
    let payload_offsets_bytes: usize = total_values * mem::size_of::<u32>();
    let new_analytic_bytes =
        analytic_btreemap_bytes::<usize, ChunkEntry>(chunks_total) + payload_offsets_bytes;

    // OLD: per-value DashMap (the shipped shape).
    let rss_before_old = read_rss_bytes();
    let old_build_start = Instant::now();
    let old_map = build_per_value_dashmap(&heaps);
    let old_build_elapsed = old_build_start.elapsed();
    let rss_after_old = read_rss_bytes();
    let old_rss_delta = match (rss_before_old, rss_after_old) {
        (Some(b), Some(a)) => Some(a.saturating_sub(b)),
        _ => None,
    };
    // DashMap = sharded hashbrown; same per-entry cost as std HashMap.
    let old_analytic_bytes = analytic_hashmap_bytes::<usize, (HeapRefId, u32)>(total_values);

    // Stride across heaps/bumps to avoid "all hits in one chunk".
    let mut sample_ptrs: Vec<usize> = Vec::with_capacity(NUM_LOOKUPS);
    'fill: {
        for (_, _, ptrs) in &heaps {
            for (i, &p) in ptrs.iter().enumerate() {
                if i % 4 == 0 {
                    sample_ptrs.push(p);
                    if sample_ptrs.len() == NUM_LOOKUPS {
                        break 'fill;
                    }
                }
            }
        }
    }

    // Accumulate into `black_box` so the loop body can't be elided.
    let new_lookup_start = Instant::now();
    let mut new_acc: u64 = 0;
    for &p in &sample_ptrs {
        let (_h, idx) = state
            .lookup_ptr(p)
            .expect("sample pointer should be in chunk index");
        new_acc = new_acc.wrapping_add(idx as u64);
    }
    let new_lookup_elapsed = new_lookup_start.elapsed();
    std::hint::black_box(new_acc);

    let old_lookup_start = Instant::now();
    let mut old_acc: u64 = 0;
    for &p in &sample_ptrs {
        let r = old_map
            .get(&p)
            .expect("sample pointer should be in DashMap");
        let (_h, idx) = *r;
        old_acc = old_acc.wrapping_add(idx as u64);
    }
    let old_lookup_elapsed = old_lookup_start.elapsed();
    std::hint::black_box(old_acc);

    let new_lookup_ns = new_lookup_elapsed.as_nanos() / (sample_ptrs.len() as u128).max(1);
    let old_lookup_ns = old_lookup_elapsed.as_nanos() / (sample_ptrs.len() as u128).max(1);

    drop(old_map);

    eprintln!(
        "chunk-index memory + speed bench:\n  \
         heaps={NUM_HEAPS} values_per_heap={VALUES_PER_HEAP} total_values={total_values}\n  \
         chunks_total={chunks_total}  chunk_entry_size={}B  values-per-chunk≈{:.1}\n  \
         NEW state (chunk-index BTreeMap):\n    \
           build_elapsed  = {new_build_elapsed:?}\n    \
           analytic_bytes ≈ {new_analytic_bytes}B  ({:.1}KiB)\n    \
           rss_delta      = {:?}\n    \
           lookup_avg     = {new_lookup_ns}ns ({} lookups in {new_lookup_elapsed:?})\n  \
         OLD state (per-value DashMap — the shipped shape):\n    \
           build_elapsed  = {old_build_elapsed:?}\n    \
           analytic_bytes ≈ {old_analytic_bytes}B  ({:.1}KiB)\n    \
           rss_delta      = {:?}\n    \
           lookup_avg     = {old_lookup_ns}ns ({} lookups in {old_lookup_elapsed:?})\n  \
         memory ratio (old / new) analytic ≈ {:.1}×\n  \
         build  ratio (new / old) wall      ≈ {:.2}×\n  \
         lookup ratio (new / old) wall      ≈ {:.2}×",
        mem::size_of::<ChunkEntry>(),
        total_values as f64 / chunks_total.max(1) as f64,
        new_analytic_bytes as f64 / 1024.0,
        new_rss_delta,
        sample_ptrs.len(),
        old_analytic_bytes as f64 / 1024.0,
        old_rss_delta,
        sample_ptrs.len(),
        old_analytic_bytes as f64 / new_analytic_bytes.max(1) as f64,
        new_build_elapsed.as_nanos() as f64 / old_build_elapsed.as_nanos().max(1) as f64,
        new_lookup_ns as f64 / old_lookup_ns.max(1) as f64,
    );

    // Per-value indexing would have chunks_total == total_values; gate
    // against regressing to that shape.
    assert!(
        chunks_total * 10 <= total_values,
        "chunk-index regression: chunks_total={chunks_total} should be <= 10% of {total_values}"
    );
    assert!(
        old_analytic_bytes >= 8 * new_analytic_bytes,
        "memory ratio collapsed: old={old_analytic_bytes}B new={new_analytic_bytes}B"
    );
    // O(log C + V_c) per lookup; 500 µs leaves headroom for debug + CI.
    assert!(
        new_lookup_ns < 500_000,
        "lookup slowdown: new avg={new_lookup_ns}ns exceeds 500µs gate"
    );

    // RSS too noisy to gate (page-granular + allocator slack); print only.
    if let (Some(new_rss), Some(old_rss)) = (new_rss_delta, old_rss_delta) {
        eprintln!(
            "  RSS-vs-analytic agreement:  new {:.2}×  old {:.2}×",
            new_rss as f64 / new_analytic_bytes.max(1) as f64,
            old_rss as f64 / old_analytic_bytes.max(1) as f64,
        );
    }
}

/// Every registered header pointer must round-trip through `lookup_ptr`
/// to its expected `(heap_id, value_index)`. Smaller sample than the
/// memory bench because each ptr gets asserted individually.
#[test]
fn test_chunk_index_lookup_is_correct_and_fast() {
    const NUM_HEAPS: usize = 50;
    const VALUES_PER_HEAP: usize = 150;

    let heaps = build_synthetic_heaps(NUM_HEAPS, VALUES_PER_HEAP);

    let state = Arc::new(StarlarkSerState::new());
    for (heap_ref, _, _) in &heaps {
        state
            .ensure_chunk_index_registered(heap_ref.heap_arc())
            .expect("register chunk index");
    }

    let lookup_start = Instant::now();
    let mut total_lookups: usize = 0;
    for (_, heap_id, ptrs) in &heaps {
        for (expected_index, &raw_ptr) in ptrs.iter().enumerate() {
            let (got_heap, got_index) = state
                .lookup_ptr(raw_ptr)
                .unwrap_or_else(|| panic!("ptr {raw_ptr:#x} missing from chunk index"));
            assert_eq!(got_heap, *heap_id, "ptr {raw_ptr:#x} routed to wrong heap");
            assert_eq!(got_index, expected_index as u32);
            total_lookups += 1;
        }
    }
    let lookup_elapsed = lookup_start.elapsed();
    let avg_ns = lookup_elapsed.as_nanos() / (total_lookups as u128).max(1);

    eprintln!(
        "chunk-index lookup bench:\n  \
         total_lookups={total_lookups}  elapsed={lookup_elapsed:?}  avg={avg_ns}ns/lookup",
    );

    // Loose gate: ms-range regression == lost cache locality / lock contention.
    assert!(
        avg_ns < 50_000,
        "lookup slowdown: avg={avg_ns}ns exceeds 50µs gate"
    );
}

#[test]
fn test_lookup_misses_when_ptr_not_in_any_chunk() {
    let heaps = build_synthetic_heaps(4, 32);

    let state = Arc::new(StarlarkSerState::new());
    for (heap_ref, _, _) in &heaps {
        state
            .ensure_chunk_index_registered(heap_ref.heap_arc())
            .expect("register chunk index");
    }

    // Tiny sentinel address — well outside any plausible heap chunk.
    assert!(state.lookup_ptr(0x42).is_none());

    let max_chunk_end = heaps
        .iter()
        .flat_map(|(h, _, _)| h.heap_arc().build_chunk_index().into_iter())
        .map(|c| c.base.addr() + c.size as usize)
        .max()
        .unwrap_or(0);
    assert!(state.lookup_ptr(max_chunk_end + 1).is_none());
}
