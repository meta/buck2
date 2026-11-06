/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::TypeId;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use dupe::Dupe;
use either::Either;
use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseDyn;
use crate::arc_erase::ArcSerializeOutcome;
use crate::arc_erase::WeakEraseDyn;
use crate::hashers::TypeIdDashMap;
use crate::hashers::TypeIdHasher;
use crate::storage::data::DataKey;
use crate::storage::data::PagableData;
use crate::storage::support::SerializerForPaging;
use crate::traits::StorageContext;

/// Shards for the binding map. It changes mostly in bulk, at page-out and at
/// the sweep after it, and page-ins of one key are already serialized by
/// [`DeserializedArcCache::get_or_page_in`], so a few shards are enough and
/// the rounding waste of many power-of-two tables is not paid.
const SHARDS: usize = 64;

/// Thread-safe cache of arcs keyed by `(TypeId, DataKey)`.
///
/// Each key is bound to at most one allocation, held weakly: the cache hands
/// out the identity every live referrer shares without keeping it resident.
/// Once the allocation is dropped, the next page-in binds a fresh one and
/// [`evict_expired`](Self::evict_expired) discards the stale entry.
///
/// A bound key costs its map bucket and the boxed weak, with no allocation of
/// its own: a page-out of a large graph leaves millions of bindings behind.
pub struct DeserializedArcCache {
    map: DashMap<(TypeId, DataKey), Slot>,
    next_binding_order: AtomicU64,
}

/// What [`DeserializedArcCache::evict_expired`] did. Concurrent page-ins can bind and rebind
/// keys while it runs, so the counts describe this call, not the cache.
#[derive(Debug, Default)]
pub struct EvictExpiredStats {
    /// Passes over the map, including the last one, which removes nothing.
    pub passes: usize,
    /// Bindings visited across all passes.
    pub entries_examined: usize,
    pub removed: usize,
    pub elapsed: Duration,
}

/// What a `(TypeId, DataKey)` holds: its binding, or the gate of the page-in
/// about to bind it. Concurrent page-ins of one key wait on the gate, so the
/// key deserializes at most once between them.
enum Slot {
    Bound {
        weak: Box<dyn WeakEraseDyn>,
        /// When the key was bound, so eviction can visit newer bindings first. Only a hint:
        /// rebinding a key out of order costs extra passes, never correctness.
        order: u64,
    },
    PagingIn(Arc<Mutex<()>>),
}

impl Slot {
    fn live(&self) -> Option<Box<dyn ArcEraseDyn>> {
        match self {
            Slot::Bound { weak, .. } => weak.upgrade(),
            Slot::PagingIn(_) => None,
        }
    }

    fn expired(&self) -> bool {
        matches!(self, Slot::Bound { weak, .. } if weak.is_expired())
    }
}

/// Per-allocation serialization slots for one type view.
type IdentityDashMap = DashMap<usize, Arc<ArcSerSlot>>;

/// Completed and in-progress Arc serializations shared across page-out workers.
pub struct ArcSerCache {
    /// One allocation can be serialized through multiple type views, such as
    /// `Arc<T>` and `Arc<dyn Trait>`. They share a pointer identity but have
    /// different wire formats, so each view needs a distinct slot. Partitioning
    /// by type also avoids storing a `TypeId` in every per-allocation key.
    by_type: TypeIdDashMap<Arc<IdentityDashMap>>,
    /// Arcs written during this page-out, waiting to be bound to their key
    /// until the rows the key depends on have committed. Bound earlier, a later
    /// page-out would reference a row that a discarded queue never wrote.
    /// Workers push here; `bind_covered` moves entries to `waiting` before it
    /// scans them, so a scan of everything waiting never holds up a worker.
    unbound: Mutex<Vec<UnboundArc>>,
    waiting: Mutex<Vec<UnboundArc>>,
}

type UnboundArc = (Box<dyn ArcEraseDyn>, DataKey, WriteTicket);

impl ArcSerCache {
    pub fn new() -> Self {
        Self {
            by_type: TypeIdDashMap::default(),
            unbound: Mutex::new(Vec::new()),
            waiting: Mutex::new(Vec::new()),
        }
    }

    /// Removes and returns the written arcs whose rows `frontier` covers, for
    /// the caller to bind with [`PagableStorage::associate_arc_with_data_key`].
    pub fn take_covered(&self, frontier: &CommitFrontier) -> Vec<(Box<dyn ArcEraseDyn>, DataKey)> {
        let mut waiting = self.waiting.lock();
        waiting.append(&mut self.unbound.lock());
        waiting
            .extract_if(.., |(_, _, ticket)| frontier.covers(ticket))
            .map(|(arc, key, _)| (arc, key))
            .collect()
    }

    /// Binds every written arc whose rows `frontier` covers to its key.
    pub fn bind_covered<S: PagableStorage + ?Sized>(&self, storage: &S, frontier: &CommitFrontier) {
        for (arc, key) in self.take_covered(frontier) {
            storage.associate_arc_with_data_key(&*arc, key);
        }
    }

    /// Forgets the arcs still waiting to be bound, for a page-out whose queued
    /// rows are being discarded.
    pub fn discard_unbound(&self) {
        self.unbound.lock().clear();
        self.waiting.lock().clear();
    }

    fn by_identity(&self, type_id: TypeId) -> Arc<IdentityDashMap> {
        match self.by_type.get(&type_id) {
            Some(entry) => entry.dupe(),
            None => self
                .by_type
                .entry(type_id)
                .or_insert_with(|| Arc::new(IdentityDashMap::default()))
                .dupe(),
        }
    }

    pub fn len(&self) -> usize {
        self.by_type.iter().map(|entry| entry.value().len()).sum()
    }
}

/// One worker's view of a shared [`ArcSerCache`].
///
/// Memoizes the `TypeId` → per-type map resolution: per-type maps are only
/// ever inserted into `by_type`, never replaced, so a resolved handle stays
/// valid for the life of the cache and repeat lookups for a type skip the
/// concurrent outer map (and the refcount traffic of duping its `Arc`).
struct ArcSerCacheLocal<'a> {
    shared: &'a ArcSerCache,
    by_type: HashMap<TypeId, Arc<IdentityDashMap>, BuildHasherDefault<TypeIdHasher>>,
}

impl<'a> ArcSerCacheLocal<'a> {
    fn new(shared: &'a ArcSerCache) -> Self {
        Self {
            shared,
            by_type: HashMap::default(),
        }
    }

    fn get_or_insert(&mut self, arc: &dyn ArcEraseDyn) -> Arc<ArcSerSlot> {
        let type_id = arc.as_arc_any().type_id();
        let Self { shared, by_type } = self;
        by_type
            .entry(type_id)
            .or_insert_with(|| shared.by_identity(type_id))
            .entry(arc.identity())
            .or_insert_with(|| Arc::new(ArcSerSlot::new()))
            .dupe()
    }
}

impl DeserializedArcCache {
    pub fn new() -> Self {
        Self {
            map: DashMap::with_shard_amount(SHARDS),
            next_binding_order: AtomicU64::new(0),
        }
    }

    /// The live allocation bound to `(type_id, key)`, if any.
    pub fn get(&self, type_id: &TypeId, key: &DataKey) -> Option<Box<dyn ArcEraseDyn>> {
        self.map.get(&(*type_id, *key))?.live()
    }

    /// Returns the allocation bound to `(type_id, key)`, or binds the one
    /// `page_in` produces. Concurrent callers for one key run `page_in` at most
    /// once between them; the rest block and share its result.
    pub fn get_or_page_in<E>(
        &self,
        type_id: TypeId,
        key: DataKey,
        page_in: impl FnOnce() -> Result<Box<dyn ArcEraseDyn>, E>,
    ) -> Result<Box<dyn ArcEraseDyn>, E> {
        let gate = Arc::new(Mutex::new(()));
        // Locked before it is published and until the binding is, so waiters
        // block on it and find the binding when they retry.
        let held = gate.lock();
        loop {
            let replaced = match self.map.entry((type_id, key)) {
                Entry::Occupied(mut occupied) => {
                    if let Some(live) = occupied.get().live() {
                        return Ok(live);
                    }
                    if let Slot::PagingIn(other) = occupied.get() {
                        let other = other.dupe();
                        drop(occupied);
                        // Wait for the holder, then look again. Never this
                        // thread's own gate: a key is the hash of its row,
                        // nested keys included, so no value can reference
                        // itself and `page_in` cannot reach the same key.
                        drop(other.lock());
                        continue;
                    }
                    Some(occupied.insert(Slot::PagingIn(gate.dupe())))
                }
                Entry::Vacant(vacant) => {
                    vacant.insert(Slot::PagingIn(gate.dupe()));
                    None
                }
            };
            // Dropped outside the shard lock: a strong-as-weak binding frees its value.
            drop(replaced);
            break;
        }
        let result = page_in().map(|arc| self.bind_unless_live(type_id, key, arc));
        if result.is_err() {
            // Unless a page-out bound the key meanwhile, leave it to the next caller.
            self.map.remove_if(
                &(type_id, key),
                |_, slot| matches!(slot, Slot::PagingIn(own) if Arc::ptr_eq(own, &gate)),
            );
        }
        drop(held);
        result
    }

    /// Binds `arc` to `(type_id, key)` and returns it, unless a live allocation
    /// was bound meanwhile, in which case that one is returned instead: a
    /// page-out registered it from a resident object graph, while nothing but
    /// the caller holds `arc` yet. Keys are content hashes, so two allocations
    /// of one key are not a fault, but handing out both would split identity.
    fn bind_unless_live(
        &self,
        type_id: TypeId,
        key: DataKey,
        arc: Box<dyn ArcEraseDyn>,
    ) -> Box<dyn ArcEraseDyn> {
        let previous = match self.map.entry((type_id, key)) {
            Entry::Occupied(mut occupied) => {
                if let Some(live) = occupied.get().live() {
                    drop(occupied);
                    return live;
                }
                Some(occupied.insert(self.bound_slot(&*arc)))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(self.bound_slot(&*arc));
                None
            }
        };
        // Dropped outside the shard lock: a strong-as-weak binding frees its value.
        drop(previous);
        arc
    }

    /// Replaces any earlier binding of `(type_id, key)`: the newest resident
    /// allocation is the one live object graphs hold, so page-in must hand out
    /// that identity rather than an older allocation that happens to survive.
    fn bind(&self, type_id: TypeId, key: DataKey, arc: &dyn ArcEraseDyn) {
        // Dropped outside the shard lock: a strong-as-weak binding frees its value.
        let previous = self.map.insert((type_id, key), self.bound_slot(arc));
        drop(previous);
    }

    fn bound_slot(&self, arc: &dyn ArcEraseDyn) -> Slot {
        Slot::Bound {
            weak: arc.downgrade(),
            order: self.next_binding_order.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Strong references to every live allocation of type `T` bound to a key.
    pub(crate) fn snapshot<T: ArcErase>(&self) -> Vec<T> {
        let type_id = TypeId::of::<T>();
        self.map
            .iter()
            .filter_map(|entry| {
                if entry.key().0 != type_id {
                    return None;
                }
                let arc = entry.value().live()?;
                Some(
                    arc.as_arc_any()
                        .downcast_ref::<T>()
                        .expect("deserialized Arc cache key must match its value type")
                        .dupe_strong(),
                )
            })
            .collect()
    }

    /// Binds `arc` to `key` when `arc` records `key` as its stored
    /// representation, so a later page-in of `key` reuses this allocation.
    pub fn register_resident(&self, key: DataKey, arc: &dyn ArcEraseDyn) {
        if arc.data_key() != Some(key) {
            return;
        }
        self.bind(arc.as_arc_any().type_id(), key, arc);
    }

    /// Drops entries whose allocation is gone, or that the cache alone keeps
    /// alive. A pass visits every entry, so this runs after a page-out or an
    /// eviction, where values drop in bulk, not per command.
    pub fn evict_expired(&self) -> EvictExpiredStats {
        let started = Instant::now();
        let mut stats = EvictExpiredStats::default();
        // Dropping a strong-as-weak binding frees its value, which may leave
        // the arcs inside it held by the cache alone, so passes repeat until
        // one frees nothing.
        loop {
            let bindings = self.bindings_in_eviction_order();
            stats.passes += 1;
            stats.entries_examined += bindings.len();
            let removed = self.remove_expired(bindings.into_iter().map(|(_, key)| key));
            stats.removed += removed;
            if removed == 0 {
                break;
            }
        }
        stats.elapsed = started.elapsed();
        stats
    }

    fn bindings_in_eviction_order(&self) -> Vec<(u64, (TypeId, DataKey))> {
        // Eager page-ins bind children before parents. Visiting the newest
        // bindings first lets dropping a parent make its children eligible in
        // this pass. Only keys and order are copied: another strong-as-weak
        // handle would itself prevent an allocation from becoming unique.
        let mut bindings: Vec<_> = self
            .map
            .iter()
            .filter_map(|entry| match entry.value() {
                Slot::Bound { order, .. } => Some((*order, *entry.key())),
                Slot::PagingIn(_) => None,
            })
            .collect();
        bindings.sort_unstable_by_key(|(order, _)| Reverse(*order));
        bindings
    }

    fn remove_expired(&self, keys: impl IntoIterator<Item = (TypeId, DataKey)>) -> usize {
        keys.into_iter()
            // A page-in may rebind after the snapshot. Eligibility belongs to
            // the current slot, never its snapshotted order or allocation.
            .filter_map(|key| self.map.remove_if(&key, |_, slot| slot.expired()))
            // Drop outside the shard lock and before visiting the next key:
            // a strong-as-weak value may be its children's last referrer.
            .map(|(_, slot)| drop(slot))
            .count()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn clear(&self) {
        self.map.clear();
    }
}

/// Where a stored row sits in its backend's commit order: a sequence number
/// and the shards that must have committed past it. A row's ticket covers the
/// rows it references too, so a value is durable once [`CommitFrontier::covers`]
/// its ticket.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WriteTicket {
    shards: u64,
    seq: u64,
}

impl WriteTicket {
    /// A row the backend wrote through before returning, or one that already existed.
    pub const DURABLE: WriteTicket = WriteTicket { shards: 0, seq: 0 };

    /// Shards are a bit each; a backend with more must check this where it
    /// chooses its shard count.
    pub const MAX_SHARDS: usize = u64::BITS as usize;

    /// The `seq`th row enqueued across the backend, landing in `shard`.
    pub fn new(shard: usize, seq: u64) -> Self {
        assert!(
            shard < Self::MAX_SHARDS,
            "write tickets track at most {} shards",
            Self::MAX_SHARDS
        );
        WriteTicket {
            shards: 1 << shard,
            seq,
        }
    }

    /// A ticket covered only when both are.
    pub fn join(self, other: WriteTicket) -> WriteTicket {
        WriteTicket {
            shards: self.shards | other.shards,
            seq: self.seq.max(other.seq),
        }
    }
}

/// The backend's commit progress as of when it was taken: per shard, the
/// sequence number below which every row has committed. Rows enqueued after
/// the frontier was taken have larger sequence numbers and are never covered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitFrontier {
    committed_before: Vec<u64>,
}

impl CommitFrontier {
    /// Everything ever enqueued has been committed, and always will have been.
    pub fn everything() -> Self {
        Self::new(Vec::new())
    }

    pub fn new(committed_before: Vec<u64>) -> Self {
        CommitFrontier { committed_before }
    }

    /// Whether every row `ticket` depends on had committed when the frontier
    /// was taken.
    pub fn covers(&self, ticket: &WriteTicket) -> bool {
        let mut shards = ticket.shards;
        while shards != 0 {
            let shard = shards.trailing_zeros() as usize;
            shards &= shards - 1;
            let committed_before = self.committed_before.get(shard).copied();
            if committed_before.unwrap_or(u64::MAX) <= ticket.seq {
                return false;
            }
        }
        true
    }
}

/// Trait for storage backends that can persist and retrieve paged-out data.
///
/// Implement this trait to provide a custom storage backend for the pagable framework.
/// The storage supports two primary use cases:
/// - Local paging: evicting and reloading data from disk when memory pressure occurs
/// - Remote graph hydration: fetching pre-serialized graphs from remote storage with
///   optional caching of deserialized arcs to avoid repeated deserialization overhead
///
/// # Methods
///
/// - [`fetch_arc_or_data_blocking`](Self::fetch_arc_or_data_blocking): Try to fetch either
///   a cached deserialized arc or raw data synchronously
/// - [`fetch_data`](Self::fetch_data): Fetch raw serialized data asynchronously
/// - [`schedule_for_paging`](Self::schedule_for_paging): Schedule an arc for eviction to storage
#[async_trait::async_trait]
pub trait PagableStorage: Send + Sync + 'static {
    /// Returns the deserialized arcs cache for this storage backend.
    fn arc_cache(&self) -> &DeserializedArcCache;

    /// Associate a stored representation with its resident arc.
    ///
    /// The weak registration lets a later page-in reuse the exact allocation
    /// without keeping it alive or deserializing the stored representation.
    fn associate_arc_with_data_key(&self, arc: &dyn ArcEraseDyn, key: DataKey) {
        arc.set_data_key(key);
        self.arc_cache().register_resident(key, arc);
        self.storage_context().notify_arc_data_key(arc, key);
    }

    /// Attempts to fetch either a cached deserialized arc or raw data synchronously.
    fn fetch_arc_or_data_blocking(
        &self,
        type_id: &TypeId,
        key: &DataKey,
    ) -> anyhow::Result<Either<Box<dyn ArcEraseDyn>, Arc<PagableData>>> {
        if let Some(arc) = self.arc_cache().get(type_id, key) {
            return Ok(Either::Left(arc));
        }
        self.fetch_data_blocking(key).map(Either::Right)
    }

    /// Fetches raw serialized data synchronously.
    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>>;

    /// Fetches raw serialized data asynchronously.
    async fn fetch_data(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>>;

    /// Schedules a type-erased arc for background paging to storage.
    ///
    /// Called when a pagable arc becomes fully unpinned and eligible for eviction.
    fn schedule_for_paging(&self, arc: Box<dyn ArcEraseDyn>);

    /// Access state owned by this storage backend.
    fn storage_context(&self) -> &StorageContext;

    /// Stores a single content-addressable [`PagableData`] blob and returns its
    /// [`DataKey`]. The key is derived from the data via
    /// `PagableData::compute_key`; if the same data is stored twice the second
    /// write is expected to be idempotent (or skipped).
    ///
    /// Implementations may buffer writes internally and defer the actual I/O
    /// until [`flush`](Self::flush) is called.
    fn store_data(&self, data: PagableData) -> anyhow::Result<DataKey> {
        self.store_data_ticketed(data).map(|(key, _ticket)| key)
    }

    /// [`store_data`](Self::store_data), also returning where the row sits in
    /// the backend's commit order. Backends that write through return
    /// [`WriteTicket::DURABLE`].
    fn store_data_ticketed(&self, data: PagableData) -> anyhow::Result<(DataKey, WriteTicket)>;

    /// How far each shard's commits have progressed; see [`CommitFrontier`].
    /// Backends that write through return [`CommitFrontier::everything`].
    fn commit_frontier(&self) -> CommitFrontier;

    /// Drops every row stored but not yet committed, cutting a write in
    /// progress short at its next statement. Only sound while nothing refers
    /// to those rows: no value evicted against them and no arc bound to them.
    /// Backends that write through have nothing to drop.
    fn discard_unwritten(&self);

    /// Commit any buffered writes to persistent storage.
    ///
    /// Callers should invoke this after a batch of `store_data` calls to
    /// ensure all data is durably written. The default implementation is a
    /// no-op (for backends that write immediately in `store_data`).
    fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Release memory held by the storage backend (e.g. SQLite page cache).
    fn release_memory(&self) {}

    /// Stores a previously-serialized item (and its transitively reachable arcs)
    /// to storage and returns its content-addressable [`DataKey`].
    ///
    /// The caller is responsible for the initial serialization: obtain
    /// `storage_context()`, build a [`SerializerForPaging`], serialize the value,
    /// `.finish()` to obtain `(item_data, item_arcs)`, then pass them in here
    /// along with the same `&StorageContext` (this method uses it to
    /// recursively serialize nested arcs).
    ///
    /// `finished` is a cache of arc serialization identity → `ArcSerSlot` shared
    /// across workers to prevent duplicate serialization.
    ///
    /// Returns `Err(PageOutError::Failed(e))` when this worker
    /// hit the original error, or `Err(PageOutError::AlreadyFailed)`
    /// when a nested arc failed in another worker.
    fn page_out_item(
        &self,
        item_data: Vec<u8>,
        item_arcs: Vec<Box<dyn ArcEraseDyn>>,
        finished: &ArcSerCache,
        storage_context: &StorageContext,
    ) -> Result<DataKey, PageOutError> {
        let (key, _ticket) =
            self.page_out_item_ticketed(item_data, item_arcs, finished, storage_context)?;
        finished.bind_covered(self, &self.commit_frontier());
        Ok(key)
    }

    /// [`page_out_item`](Self::page_out_item), also returning a ticket that
    /// covers the item's row and every arc row it references, including those
    /// another worker wrote during this page-out. The arcs it writes are left
    /// in `finished` until the caller binds them with
    /// [`ArcSerCache::bind_covered`] once their rows have committed.
    fn page_out_item_ticketed(
        &self,
        item_data: Vec<u8>,
        item_arcs: Vec<Box<dyn ArcEraseDyn>>,
        finished: &ArcSerCache,
        storage_context: &StorageContext,
    ) -> Result<(DataKey, WriteTicket), PageOutError> {
        enum Task {
            Start {
                arc: Box<dyn ArcEraseDyn>,
                slot: Arc<ArcSerSlot>,
            },
            Finish {
                arc: Box<dyn ArcEraseDyn>,
                slot: Arc<ArcSerSlot>,
                data: Vec<u8>,
                child_slots: Vec<Arc<ArcSerSlot>>,
            },
        }

        // Slots resolve once per graph edge — here and where child arcs are
        // extracted below — and then travel with their task, keeping the
        // claim, finish, and resolve stages free of concurrent map probes.
        let mut cache = ArcSerCacheLocal::new(finished);
        let item_slots: Vec<Arc<ArcSerSlot>> = item_arcs
            .iter()
            .map(|arc| cache.get_or_insert(&**arc))
            .collect();

        // Pushed once per item: a page-out writes about twenty arc rows per
        // value, and a push per arc from every worker contends on one lock.
        // Holds every arc this item's row references, written here or by
        // another item still in progress, so that once this item is handed
        // back and its value evicted, nothing its row references is unbound.
        let mut written = Vec::new();
        let mut tasks: Vec<Task> = item_arcs
            .iter()
            .zip(&item_slots)
            .map(|(arc, slot)| Task::Start {
                arc: arc.clone_dyn(),
                slot: slot.dupe(),
            })
            .collect();

        while let Some(task) = tasks.pop() {
            match task {
                Task::Start { arc, slot } => {
                    if !slot.try_claim() {
                        let Some((key, ticket)) = slot.wait() else {
                            return Err(PageOutError::AlreadyFailed);
                        };
                        written.push((arc, key, ticket));
                        continue;
                    }

                    if let Some(key) = arc.data_key() {
                        self.associate_arc_with_data_key(&*arc, key);
                        slot.set_success(key, WriteTicket::DURABLE);
                        continue;
                    }

                    let mut serializer = SerializerForPaging::new(storage_context);
                    let (data, arcs) = match arc.serialize(&mut serializer) {
                        Ok(ArcSerializeOutcome::Serialized) => serializer.finish(),
                        Ok(ArcSerializeOutcome::ReuseDataKey(key)) => {
                            self.associate_arc_with_data_key(&*arc, key);
                            slot.set_success(key, WriteTicket::DURABLE);
                            continue;
                        }
                        Err(e) => {
                            slot.set_failed();
                            return Err(PageOutError::Failed(e));
                        }
                    };

                    let child_slots: Vec<Arc<ArcSerSlot>> =
                        arcs.iter().map(|arc| cache.get_or_insert(&**arc)).collect();
                    let subtasks: Vec<_> = arcs
                        .iter()
                        .zip(&child_slots)
                        .filter(|(_, s)| s.result.get().copied().flatten().is_none())
                        .map(|(arc, s)| Task::Start {
                            arc: arc.clone_dyn(),
                            slot: s.dupe(),
                        })
                        .collect();

                    tasks.push(Task::Finish {
                        arc,
                        slot,
                        data,
                        child_slots,
                    });
                    tasks.extend(subtasks);
                }
                Task::Finish {
                    arc,
                    slot,
                    data,
                    child_slots,
                } => match resolve_and_store(self, data, &child_slots) {
                    Ok((key, ticket)) => {
                        slot.set_success(key, ticket);
                        written.push((arc, key, ticket));
                    }
                    Err(e) => {
                        slot.set_failed();
                        return Err(e);
                    }
                },
            }
        }

        if !written.is_empty() {
            finished.unbound.lock().append(&mut written);
        }
        resolve_and_store(self, item_data, &item_slots)
    }
}

fn resolve_and_store(
    storage: &(impl PagableStorage + ?Sized),
    data: Vec<u8>,
    child_slots: &[Arc<ArcSerSlot>],
) -> Result<(DataKey, WriteTicket), PageOutError> {
    let mut keys = Vec::with_capacity(child_slots.len());
    let mut ticket = WriteTicket::DURABLE;
    for slot in child_slots {
        let Some((key, child_ticket)) = slot.wait() else {
            return Err(PageOutError::AlreadyFailed);
        };
        keys.push(key);
        ticket = ticket.join(child_ticket);
    }
    let (key, own) = storage
        .store_data_ticketed(PagableData { data, arcs: keys })
        .map_err(PageOutError::Failed)?;
    Ok((key, ticket.join(own)))
}

#[derive(Debug, thiserror::Error)]
pub enum PageOutError {
    /// Item being paged out hit an error (serialization or storage).
    #[error("page-out failed: {0}")]
    Failed(anyhow::Error),
    /// Arc failed to serialize on another worker, error already propagated.
    #[error("page-out already failed on another worker")]
    AlreadyFailed,
}

/// Slot for tracking an in-progress or completed arc serialization.
pub struct ArcSerSlot {
    /// Set to `true` by the first thread to claim this arc.
    claimed: AtomicBool,
    /// The key and the ticket covering the arc's row and its children on
    /// success, `None` on failure.
    result: OnceCell<Option<(DataKey, WriteTicket)>>,
}

impl ArcSerSlot {
    fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            result: OnceCell::new(),
        }
    }

    /// Try to claim this slot. Returns `true` if this thread won.
    fn try_claim(&self) -> bool {
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    /// Block until the result is available.
    /// Returns the key and ticket on success, `None` if serialization failed.
    fn wait(&self) -> Option<(DataKey, WriteTicket)> {
        *self.result.wait()
    }

    fn set_success(&self, key: DataKey, ticket: WriteTicket) {
        let _ = self.result.set(Some((key, ticket)));
    }

    fn set_failed(&self) {
        let _ = self.result.set(None);
    }
}

#[cfg(test)]
mod tests;
