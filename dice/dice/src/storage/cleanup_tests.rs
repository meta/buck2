/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use allocative::Allocative;
use async_trait::async_trait;
use derive_more::Display;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use pagable::DataKey;
use pagable::Pagable;
use pagable::StorageContext;
use pagable::arc_erase::ArcEraseDyn;
use pagable::pagable_typetag;
use pagable::storage::data::PagableData;
use pagable::storage::in_memory::InMemoryPagableStorage;
use pagable::storage::traits::CommitFrontier;
use pagable::storage::traits::DeserializedArcCache;
use pagable::storage::traits::PagableStorage;
use pagable::storage::traits::WriteTicket;
use parking_lot::Mutex;

use crate::DiceKeyDyn;
use crate::api::computations::DiceComputations;
use crate::api::cycles::DetectCycles;
use crate::api::key::EqualityBehavior;
use crate::api::key::Key;
use crate::api::key::PagableValueSerialize;
use crate::api::key::ValueSerialize;
use crate::dice::Dice;
use crate::metrics::EvictionSnapshot;
use crate::metrics::PagingMemorySnapshot;
use crate::metrics::set_thread_alloc_counters;
use crate::storage::DiceStorage;
use crate::storage::EvictionPolicy;

thread_local! {
    // The installed reader must not enable accounting for unrelated tests or DICE actors.
    static ALLOC_COUNTERS: Cell<Option<(u64, u64)>> = const { Cell::new(None) };
}

fn fake_alloc_counters() -> Option<(u64, u64)> {
    ALLOC_COUNTERS.try_with(Cell::get).ok().flatten()
}

fn add_fake_bytes(allocated: u64, deallocated: u64) {
    let _ = ALLOC_COUNTERS.try_with(|counters| {
        if let Some((a, d)) = counters.get() {
            counters.set(Some((a + allocated, d + deallocated)));
        }
    });
}

struct FakeAllocScope {
    // Keep the scope on the thread whose counters it enables, including across awaits.
    _not_send: PhantomData<Rc<()>>,
}

impl FakeAllocScope {
    fn new() -> Self {
        set_thread_alloc_counters(fake_alloc_counters);
        ALLOC_COUNTERS.with(|counters| {
            assert_eq!(counters.replace(Some((0, 0))), None);
        });
        Self {
            _not_send: PhantomData,
        }
    }
}

impl Drop for FakeAllocScope {
    fn drop(&mut self) {
        ALLOC_COUNTERS.with(|counters| counters.set(None));
    }
}

/// An actual retained allocation whose last owner's drop reports exact fake bytes.
struct TrackedPayload {
    bytes: Box<[u8]>,
    drops: Arc<AtomicUsize>,
}

impl TrackedPayload {
    fn new(bytes: usize, drops: Arc<AtomicUsize>) -> Self {
        let bytes = vec![0; bytes].into_boxed_slice();
        add_fake_bytes(bytes.len() as u64, 0);
        Self { bytes, drops }
    }
}

impl Drop for TrackedPayload {
    fn drop(&mut self) {
        add_fake_bytes(0, self.bytes.len() as u64);
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

struct CacheRetainingStorage {
    inner: Arc<dyn PagableStorage>,
    /// Stand-in for a backend cache: dropped by `release_memory`.
    cached: Mutex<Vec<TrackedPayload>>,
    release_calls: AtomicUsize,
    drops: Arc<AtomicUsize>,
}

impl CacheRetainingStorage {
    fn new(inner: Arc<dyn PagableStorage>) -> Self {
        Self {
            inner,
            cached: Mutex::new(Vec::new()),
            release_calls: AtomicUsize::new(0),
            drops: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn cache_payload(&self, bytes: usize) {
        self.cached
            .lock()
            .push(TrackedPayload::new(bytes, self.drops.dupe()));
    }

    fn dropped_payloads(&self) -> usize {
        self.drops.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl PagableStorage for CacheRetainingStorage {
    fn arc_cache(&self) -> &DeserializedArcCache {
        self.inner.arc_cache()
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.inner.fetch_data_blocking(key)
    }

    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.inner.fetch_data(key).await
    }

    fn schedule_for_paging(&self, arc: Box<dyn ArcEraseDyn>) {
        self.inner.schedule_for_paging(arc)
    }

    fn storage_context(&self) -> &StorageContext {
        self.inner.storage_context()
    }

    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)> {
        self.inner.store_data_ticketed(data)
    }

    fn commit_frontier(&self) -> CommitFrontier {
        self.inner.commit_frontier()
    }

    fn discard_unwritten(&self) {
        self.inner.discard_unwritten()
    }

    fn flush(&self) -> anyhow::Result<()> {
        self.inner.flush()
    }

    fn release_memory(&self) {
        self.release_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.release_memory();
        self.cached.lock().clear();
    }
}

#[derive(Allocative, Clone, Dupe, Debug, Display, PartialEq, Eq, Hash, Pagable)]
#[pagable_typetag(DiceKeyDyn)]
struct CleanupKey(u32);

#[async_trait]
impl Key for CleanupKey {
    type Value = u64;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        u64::from(self.0)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::Compare(|a, b| a == b)
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        PagableValueSerialize::<Self::Value>::new()
    }
}

fn make_dice(backend: &Arc<CacheRetainingStorage>, eviction: EvictionPolicy) -> Arc<Dice> {
    let mut storage = DiceStorage::new(backend.dupe());
    storage.set_eviction_policy(eviction);
    let mut builder = Dice::builder();
    builder.set_pagable_storage(storage);
    builder.build(DetectCycles::Disabled)
}

async fn compute_keys(dice: &Arc<Dice>, count: u32) -> anyhow::Result<()> {
    let root = dice.new_root().await;
    let tx = dice.updater(root).commit().await;
    for key in 0..count {
        assert_eq!(*tx.compute(&CleanupKey(key)).await?, u64::from(key));
    }
    drop(tx);
    dice.wait_for_idle().await;
    Ok(())
}

fn memory(dice: &Dice) -> PagingMemorySnapshot {
    dice.paging_memory_metrics()
        .expect("fake allocator counters should be available on this test thread")
}

fn evictions(dice: &Dice) -> EvictionSnapshot {
    dice.eviction_metrics()
        .expect("test DICE should have paging storage")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backend_cleanup_bytes_are_counted_once_after_evicting_backed_values() -> anyhow::Result<()>
{
    let _scope = FakeAllocScope::new();
    let backing = InMemoryPagableStorage::new();
    let backend = Arc::new(CacheRetainingStorage::new(backing.handle()));
    let dice = make_dice(&backend, EvictionPolicy::OnMemoryPressure);
    compute_keys(&dice, 3).await?;
    dice.page_out().await?;
    assert_eq!(dice.pagable_node_counts().await.backed, 3);
    assert_eq!(memory(&dice).bytes_offloaded, 0);

    backend.cache_payload(8192);
    backend.cache_payload(4096);
    assert_eq!(backend.dropped_payloads(), 0);
    let before = memory(&dice);
    let before_evictions = evictions(&dice);
    assert_eq!(dice.evict_backed().await, 3);
    assert_eq!(backend.dropped_payloads(), 2);
    assert!(backend.cached.lock().is_empty());
    assert_eq!(backend.release_calls.load(Ordering::Relaxed), 2);
    assert_eq!(
        memory(&dice).since(before),
        PagingMemorySnapshot {
            bytes_offloaded: 12288,
            bytes_restored: 0,
        },
    );
    assert_eq!(
        evictions(&dice).since(before_evictions),
        EvictionSnapshot {
            evictions: 1,
            values_evicted: 3,
            bytes_evicted: Some(12288),
        },
        "backend cache bytes must not add graph values or eviction passes",
    );
    assert_eq!(dice.paged_out_node_total(), Some(3));

    let before = memory(&dice);
    let before_evictions = evictions(&dice);
    assert_eq!(dice.evict_backed().await, 0);
    assert_eq!(memory(&dice).since(before).bytes_offloaded, 0);
    assert_eq!(
        evictions(&dice).since(before_evictions),
        EvictionSnapshot {
            evictions: 1,
            values_evicted: 0,
            bytes_evicted: Some(0),
        },
    );
    assert_eq!(backend.dropped_payloads(), 2);
    assert_eq!(dice.paged_out_node_total(), Some(3));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_page_out_cleanup_does_not_charge_pressure_eviction_bytes() -> anyhow::Result<()> {
    let _scope = FakeAllocScope::new();
    let backing = InMemoryPagableStorage::new();
    let backend = Arc::new(CacheRetainingStorage::new(backing.handle()));
    let dice = make_dice(&backend, EvictionPolicy::OnIdle);
    compute_keys(&dice, 2).await?;
    backend.cache_payload(1000);
    backend.cache_payload(3000);
    assert_eq!(backend.dropped_payloads(), 0);
    let before_evictions = evictions(&dice);

    dice.page_out().await?;
    assert_eq!(backend.dropped_payloads(), 2);
    assert_eq!(backend.release_calls.load(Ordering::Relaxed), 1);
    assert_eq!(
        memory(&dice),
        PagingMemorySnapshot {
            bytes_offloaded: 4000,
            bytes_restored: 0,
        },
    );
    assert_eq!(evictions(&dice), before_evictions);
    assert_eq!(evictions(&dice).bytes_evicted, Some(0));
    assert_eq!(dice.paged_out_node_total(), Some(2));
    assert_eq!(dice.pagable_node_counts().await.paged_out, 2);

    dice.page_out().await?;
    assert_eq!(backend.release_calls.load(Ordering::Relaxed), 1);
    assert_eq!(memory(&dice).bytes_offloaded, 4000);
    assert_eq!(dice.evict_backed().await, 0);
    assert_eq!(memory(&dice).bytes_offloaded, 4000);
    assert_eq!(evictions(&dice).bytes_evicted, Some(0));
    assert_eq!(evictions(&dice).values_evicted, 0);
    Ok(())
}
