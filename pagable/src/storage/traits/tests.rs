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
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use dashmap::try_result::TryResult;
use dupe::Dupe;

use super::*;
use crate as pagable;
use crate::PagableArc;
use crate::PagableDeserialize;
use crate::PagableDeserializerRecipe;
use crate::PagableSerialize;
use crate::PagableTagged;
use crate::PartialPagableArc;
use crate::PinnedPagableArc;
use crate::arc_erase::ArcErase;
use crate::arc_erase::ArcEraseType;
use crate::arc_erase::StdArcEraseType;
use crate::storage::handle::PagableStorageHandle;
use crate::storage::in_memory::InMemoryPagableStorage;
use crate::storage::support::SerializerForPaging;
use crate::testing::TestingDeserializer;
use crate::testing::TestingSerializer;
use crate::traits::PagableDeserializer;
use crate::traits::PagableSerializer;

#[crate::pagable_typetag]
trait SerializationView: PagableTagged + Send + Sync + std::fmt::Debug {
    fn value(&self) -> u8;
}

#[derive(crate::Pagable, Debug)]
struct SerializationViewValue(u8);

#[crate::pagable_typetag]
impl SerializationView for SerializationViewValue {
    fn value(&self) -> u8 {
        self.0
    }
}

struct ResidentArcValue(u8);

impl PagableSerialize for ResidentArcValue {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        self.0.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for ResidentArcValue {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        Ok(Self(u8::pagable_deserialize(deserializer)?))
    }
}

/// Deserializations of [`ReuseProbe`], which only
/// `page_in_reuses_arc_still_resident_after_page_out` deserializes, so its
/// exact counts hold whatever the other tests do on parallel threads.
static REUSE_PROBE_DESERIALIZATIONS: AtomicUsize = AtomicUsize::new(0);

struct ReuseProbe(u8);

impl PagableSerialize for ReuseProbe {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        self.0.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for ReuseProbe {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> crate::Result<Self> {
        REUSE_PROBE_DESERIALIZATIONS.fetch_add(1, Ordering::SeqCst);
        Ok(Self(u8::pagable_deserialize(deserializer)?))
    }
}

#[derive(Clone)]
struct PausedDataKeyLookupArc {
    inner: Arc<PausedDataKeyLookupArcInner>,
}

struct PausedDataKeyLookupArcInner {
    arc: PartialPagableArc<ResidentArcValue>,
    pause_next_lookup: AtomicBool,
    lookup_started: Barrier,
    release_lookup: Barrier,
}

impl PausedDataKeyLookupArc {
    fn new(value: ResidentArcValue) -> Self {
        Self {
            inner: Arc::new(PausedDataKeyLookupArcInner {
                arc: PartialPagableArc::new(value),
                pause_next_lookup: AtomicBool::new(true),
                lookup_started: Barrier::new(2),
                release_lookup: Barrier::new(2),
            }),
        }
    }

    fn wait_for_paused_lookup(&self) {
        self.inner.lookup_started.wait();
    }

    fn release_paused_lookup(&self) {
        self.inner.release_lookup.wait();
    }
}

impl PagableSerialize for PausedDataKeyLookupArc {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        serializer.serialize_arc(self)
    }
}

impl ArcErase for PausedDataKeyLookupArc {
    type Weak = ();

    fn dupe_strong(&self) -> Self {
        self.clone()
    }

    fn downgrade(&self) -> Option<Self::Weak> {
        None
    }

    fn erase_type() -> impl ArcEraseType {
        StdArcEraseType::<Self>::new()
    }

    fn identity(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    fn set_data_key(&self, key: DataKey) {
        ArcErase::set_data_key(&self.inner.arc, key)
    }

    fn data_key(&self) -> Option<DataKey> {
        let key = self.inner.arc.data_key();
        if self.inner.pause_next_lookup.swap(false, Ordering::SeqCst) {
            self.inner.lookup_started.wait();
            self.inner.release_lookup.wait();
        }
        key
    }

    fn needs_paging_out(&self) -> bool {
        self.inner.arc.data_key().is_none()
    }

    fn serialize_inner(
        &self,
        serializer: &mut dyn PagableSerializer,
    ) -> crate::Result<ArcSerializeOutcome> {
        ArcErase::serialize_inner(&self.inner.arc, serializer)
    }

    fn deserialize_inner<'de, D: PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> crate::Result<Self> {
        unreachable!("the race-only test Arc is never deserialized")
    }
}

fn deserialize_resident_partial_arc<'de, D: PagableDeserializer<'de> + ?Sized>(
    deserializer: &mut D,
) -> crate::Result<PartialPagableArc<ResidentArcValue>> {
    deserialize_partial_arc(deserializer)
}

fn deserialize_partial_arc<'de, T, D>(deserializer: &mut D) -> crate::Result<PartialPagableArc<T>>
where
    T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + 'static,
    D: PagableDeserializer<'de> + ?Sized,
{
    fn deserialize_fn<
        T: for<'a> PagableDeserialize<'a> + PagableSerialize + Send + Sync + 'static,
    >(
        deserializer: &mut dyn PagableDeserializer<'_>,
        _recipe: Arc<dyn PagableDeserializerRecipe>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        Ok(Box::new(PartialPagableArc::new(T::pagable_deserialize(
            deserializer,
        )?)))
    }

    let arc =
        deserializer.deserialize_arc(TypeId::of::<PartialPagableArc<T>>(), deserialize_fn::<T>)?;
    arc.as_arc_any()
        .downcast_ref::<PartialPagableArc<T>>()
        .ok_or_else(|| anyhow::anyhow!("resident partial arc type mismatch"))
        .cloned()
}

/// Counts `fetch_data_blocking` and `store_data` calls per `DataKey`.
struct CountingStorage {
    inner: Arc<dyn PagableStorage>,
    fetch_count: AtomicUsize,
    store_count: AtomicUsize,
    /// Set when an arc was already visible in the cache at the moment it was keyed.
    keyed_while_visible: AtomicBool,
}

impl CountingStorage {
    fn new(inner: Arc<dyn PagableStorage>) -> Self {
        Self {
            inner,
            fetch_count: AtomicUsize::new(0),
            store_count: AtomicUsize::new(0),
            keyed_while_visible: AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl PagableStorage for CountingStorage {
    fn arc_cache(&self) -> &DeserializedArcCache {
        self.inner.arc_cache()
    }

    fn commit_frontier(&self) -> CommitFrontier {
        CommitFrontier::everything()
    }

    fn discard_unwritten(&self) {}

    fn associate_arc_with_data_key(&self, arc: &dyn ArcEraseDyn, key: DataKey) {
        if self
            .arc_cache()
            .get(&arc.as_arc_any().type_id(), &key)
            .is_some()
        {
            self.keyed_while_visible.store(true, Ordering::SeqCst);
        }
        self.inner.associate_arc_with_data_key(arc, key)
    }

    fn fetch_data_blocking(&self, key: &DataKey) -> anyhow::Result<Arc<PagableData>> {
        self.fetch_count.fetch_add(1, Ordering::SeqCst);
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
        self.store_count.fetch_add(1, Ordering::SeqCst);
        self.inner.store_data_ticketed(data)
    }

    fn flush(&self) -> anyhow::Result<()> {
        self.inner.flush()
    }
}

fn serialize_shared_arc_items(
    storage: &CountingStorage,
    num_items: usize,
) -> anyhow::Result<Vec<DataKey>> {
    let shared_arc: Arc<Vec<u8>> = Arc::new(vec![0xAB; 1000]);
    let finished = ArcSerCache::new();
    let mut keys = Vec::with_capacity(num_items);
    for i in 0..num_items {
        let storage_context = storage.storage_context();
        let mut ser = SerializerForPaging::new(storage_context);
        (i as u8).pagable_serialize(&mut ser)?;
        shared_arc.pagable_serialize(&mut ser)?;
        let (data, arcs) = ser.finish();
        keys.push(
            storage
                .page_out_item(data, arcs, &finished, storage_context)
                .map_err(|e| match e {
                    PageOutError::Failed(e) => e,
                    PageOutError::AlreadyFailed => {
                        panic!("unexpected AlreadyFailed")
                    }
                })?,
        );
    }
    storage.flush()?;
    Ok(keys)
}

/// A value whose serialization blocks until released, so a page-out item can be
/// held mid-write.
struct GatedValue {
    started: Barrier,
    release: Barrier,
}

impl GatedValue {
    fn new() -> Self {
        Self {
            started: Barrier::new(2),
            release: Barrier::new(2),
        }
    }
}

impl PagableSerialize for GatedValue {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        self.started.wait();
        self.release.wait();
        0u8.pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for GatedValue {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> crate::Result<Self> {
        Err(crate::Error::msg("a gated value is never read back"))
    }
}

fn page_out_item_ticketed(
    storage: &CountingStorage,
    finished: &ArcSerCache,
    tag: u8,
    arcs: &[&dyn PagableSerialize],
) -> anyhow::Result<DataKey> {
    let storage_context = storage.storage_context();
    let mut serializer = SerializerForPaging::new(storage_context);
    tag.pagable_serialize(&mut serializer)?;
    for arc in arcs {
        arc.pagable_serialize(&mut serializer)?;
    }
    let (data, arcs) = serializer.finish();
    let (key, _ticket) = storage
        .page_out_item_ticketed(data, arcs, finished, storage_context)
        .map_err(|error| match error {
            PageOutError::Failed(error) => error,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    Ok(key)
}

/// An item's row can reference an arc that another item is still in the middle
/// of writing. Once the item is handed back, a page-out may evict its value, so
/// everything its row references must be bindable by then: a page-in of the
/// value that found the arc unregistered would restore a second copy of it.
#[test]
fn arc_a_handed_back_item_references_is_bindable() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let finished = Arc::new(ArcSerCache::new());
    let shared = PartialPagableArc::new(ResidentArcValue(7));
    let gated: Arc<GatedValue> = Arc::new(GatedValue::new());

    // The writer's item: arcs are processed from the last serialized, so it
    // writes `shared` and then blocks in `gated` with `shared` still unpushed.
    let writer = std::thread::spawn({
        let storage = storage.dupe();
        let finished = finished.dupe();
        let shared = shared.dupe();
        let gated = gated.dupe();
        move || page_out_item_ticketed(&storage, &finished, 0, &[&gated, &shared])
    });
    gated.started.wait();

    let referrer = page_out_item_ticketed(&storage, &finished, 1, &[&shared])?;
    let shared_key = storage.fetch_data_blocking(&referrer)?.arcs[0];
    finished.bind_covered(&*storage, &storage.commit_frontier());
    let bound = storage.arc_cache().get(
        &TypeId::of::<PartialPagableArc<ResidentArcValue>>(),
        &shared_key,
    );
    assert!(
        bound.is_some_and(|arc| arc.identity() == ArcErase::identity(&shared)),
        "the referrer was handed back, but the arc its row references is not bound"
    );

    gated.release.wait();
    writer.join().expect("writer thread should not panic")?;
    Ok(())
}

fn page_out_paused_lookup_arc(
    storage: &CountingStorage,
    arc: &PausedDataKeyLookupArc,
) -> anyhow::Result<DataKey> {
    let storage_context = storage.storage_context();
    let mut serializer = SerializerForPaging::new(storage_context);
    1u8.pagable_serialize(&mut serializer)?;
    arc.pagable_serialize(&mut serializer)?;
    let (data, arcs) = serializer.finish();
    storage
        .page_out_item(data, arcs, &ArcSerCache::new(), storage_context)
        .map_err(|error| match error {
            PageOutError::Failed(error) => error,
            PageOutError::AlreadyFailed => {
                anyhow::anyhow!("a separate serialization cache cannot already be failed")
            }
        })
}

fn check_concrete_and_dyn_arc_views(dyn_first: bool) -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);

    let concrete = Arc::new(SerializationViewValue(42));
    let dyn_view: Arc<dyn SerializationView> = concrete.dupe();
    assert_eq!(
        Arc::as_ptr(&concrete) as *const (),
        Arc::as_ptr(&dyn_view) as *const (),
        "the concrete and dyn views should share one allocation",
    );

    let finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    if dyn_first {
        dyn_view.pagable_serialize(&mut ser)?;
        concrete.pagable_serialize(&mut ser)?;
    } else {
        concrete.pagable_serialize(&mut ser)?;
        dyn_view.pagable_serialize(&mut ser)?;
    }
    let (data, arcs) = ser.finish();
    let root_key = storage
        .page_out_item(data, arcs, &finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    assert_eq!(finished.len(), 2);
    storage.flush()?;

    let root = storage.fetch_data_blocking(&root_key)?;
    let (dyn_index, concrete_index) = if dyn_first { (0, 1) } else { (1, 0) };
    assert_ne!(
        root.arcs[dyn_index], root.arcs[concrete_index],
        "different serialization views must have distinct stored values",
    );
    let stored_dyn_view = storage.fetch_data_blocking(&root.arcs[dyn_index])?;
    assert_eq!(
        stored_dyn_view.arcs.as_slice(),
        &[root.arcs[concrete_index]],
        "the dyn view should reference the canonical concrete Arc",
    );
    let mut deser = handle.root_deserializer(root_key, &root);
    let (restored_dyn, restored_concrete) = if dyn_first {
        (
            Arc::<dyn SerializationView>::pagable_deserialize(&mut deser)?,
            Arc::<SerializationViewValue>::pagable_deserialize(&mut deser)?,
        )
    } else {
        let concrete = Arc::<SerializationViewValue>::pagable_deserialize(&mut deser)?;
        let dyn_view = Arc::<dyn SerializationView>::pagable_deserialize(&mut deser)?;
        (dyn_view, concrete)
    };
    assert_eq!(restored_dyn.value(), 42);
    assert_eq!(restored_concrete.0, 42);
    assert_eq!(
        Arc::as_ptr(&restored_dyn) as *const (),
        Arc::as_ptr(&restored_concrete) as *const (),
        "concrete and dyn views should restore the same Arc allocation",
    );
    Ok(())
}

/// A weak handle reaches the storage only while some strong handle keeps it open,
/// and an upgraded handle keeps it open for as long as it is held.
#[test]
fn weak_storage_handle_upgrades_only_while_storage_is_open() {
    let backing = InMemoryPagableStorage::new();
    let handle = PagableStorageHandle::new(backing.handle());
    let weak = handle.downgrade();

    let upgraded = weak.upgrade().expect("the storage is still open");
    assert!(
        std::ptr::eq(upgraded.storage_context(), handle.storage_context()),
        "upgrading returns a handle to the same storage"
    );

    drop(handle);
    drop(backing);
    assert!(
        weak.upgrade().is_some(),
        "the upgraded handle alone keeps the storage open"
    );

    drop(upgraded);
    assert!(
        weak.upgrade().is_none(),
        "no strong handle remains, so the storage is closed"
    );
}

#[test]
fn page_out_preserves_dyn_then_concrete_arc_view() -> anyhow::Result<()> {
    check_concrete_and_dyn_arc_views(true)
}

#[test]
fn page_out_preserves_concrete_then_dyn_arc_view() -> anyhow::Result<()> {
    check_concrete_and_dyn_arc_views(false)
}

/// Parallel deserialization of values sharing the same `Arc` must not
/// fetch the arc's data more than once from storage.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    all(miri, not(target_os = "linux")),
    ignore = "tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll"
)]
async fn deserialize_arc_does_not_duplicate() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));

    let num_items = 100usize;
    let item_keys = serialize_shared_arc_items(&storage, num_items)?;
    storage.fetch_count.store(0, Ordering::SeqCst);

    // Deserialize all items in parallel. Each item's deserializer calls
    // deserialize_arc for the shared Arc, which fetches inside the cache's
    // page-in gate. Without dedup, the shared arc's data is fetched num_items
    // times. Every task's arc is kept alive until all have finished: the cache
    // holds arcs weakly, so a task starting after the last referrer dropped
    // would legitimately fetch again.
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let handles: Vec<_> = item_keys
        .into_iter()
        .map(|key| {
            let storage = storage.dupe();
            let handle = handle.dupe();
            tokio::spawn(async move {
                let data = storage.fetch_data_blocking(&key)?;
                let mut deser = handle.root_deserializer(key, &data);
                let _value: u8 = crate::PagableDeserialize::pagable_deserialize(&mut deser)?;
                let values: Arc<Vec<u8>> =
                    crate::PagableDeserialize::pagable_deserialize(&mut deser)?;
                Ok::<_, anyhow::Error>(values)
            })
        })
        .collect();

    let mut values = Vec::with_capacity(num_items);
    for h in handles {
        values.push(h.await??);
    }

    let total = storage.fetch_count.load(Ordering::SeqCst);
    assert!(
        total <= num_items + 1,
        "expected at most {} fetch_data_blocking calls, got {} \
         (shared arc fetched {} extra times)",
        num_items + 1,
        total,
        total - num_items - 1,
    );
    Ok(())
}

/// `take_arc_key` then `deserialize_arc_by_key` must be `deserialize_arc`
/// in two steps: no bytes read, one slot consumed, and the same allocation
/// restored as an eager read of the same arc.
#[test]
fn take_arc_key_then_deserialize_by_key_matches_deserialize_arc() -> anyhow::Result<()> {
    fn deserialize_fn(
        deserializer: &mut dyn PagableDeserializer<'_>,
        _recipe: Arc<dyn PagableDeserializerRecipe>,
    ) -> crate::Result<Box<dyn ArcEraseDyn>> {
        Ok(Box::new(<Arc<Vec<u8>> as ArcErase>::deserialize_inner(
            deserializer,
        )?))
    }

    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let keys = serialize_shared_arc_items(&storage, 2)?;
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);

    let data = storage.fetch_data_blocking(&keys[0])?;
    let eager_data = storage.fetch_data_blocking(&keys[1])?;
    storage.fetch_count.store(0, Ordering::SeqCst);
    let mut de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    let before = de.position();
    let key = de.take_arc_key()?.expect("the item holds one arc slot");
    let after = de.position();
    assert_eq!(after.arc_index, before.arc_index + 1, "one slot consumed");
    assert_eq!(
        after.byte_pos, before.byte_pos,
        "taking a key reads no bytes"
    );
    let error = de.take_arc_key().expect_err("no slot left to take");
    assert_eq!(
        error.to_string(),
        "Arc slot index 1 out of bounds (1 slots)"
    );
    assert_eq!(
        de.position(),
        after,
        "exhaustion must not advance the cursor"
    );
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), 0);
    drop(de);

    let later = handle.deserialize_arc_by_key(key, TypeId::of::<Arc<Vec<u8>>>(), deserialize_fn)?;
    let later = later
        .as_arc_any()
        .downcast_ref::<Arc<Vec<u8>>>()
        .expect("the slot holds an Arc<Vec<u8>>")
        .dupe();
    assert_eq!(later.as_slice(), &[0xAB; 1000]);
    assert_eq!(
        storage.fetch_count.load(Ordering::SeqCst),
        1,
        "the by-key path must actually fetch and deserialize the cold arc"
    );

    let mut de = handle.root_deserializer(keys[1], &eager_data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    let eager: Arc<Vec<u8>> = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    assert!(
        Arc::ptr_eq(&eager, &later),
        "both paths must restore the one cached allocation"
    );
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn exhausted_arc_slots_preserve_cursor_and_report_context() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let data = PagableData {
        data: vec![17],
        arcs: Vec::new(),
    };
    let mut de = handle.root_deserializer(data.compute_key(), &data);
    let before = de.position();
    let error = de.take_arc_key().expect_err("there are no arc slots");
    assert_eq!(
        error.to_string(),
        "Arc slot index 0 out of bounds (0 slots)"
    );
    let error = Arc::<Vec<u8>>::pagable_deserialize(&mut de).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("Deserializing arc with type"));
    assert!(message.contains("Arc slot index 0 out of bounds (0 slots)"));
    assert_eq!(de.position(), before);
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), 0);
    assert_eq!(u8::pagable_deserialize(&mut de)?, 17);
    Ok(())
}

#[test]
fn inline_arc_key_fallback_preserves_input() -> anyhow::Result<()> {
    let value = Arc::new(vec![1u8, 2, 3]);
    let mut ser = TestingSerializer::new();
    value.pagable_serialize(&mut ser)?;
    let bytes = ser.finish();
    let mut de = TestingDeserializer::new(&bytes);
    let before = de.position();
    assert!(de.take_arc_key()?.is_none());
    assert_eq!(de.position(), before, "unsupported lookup consumes nothing");
    assert_eq!(Arc::<Vec<u8>>::pagable_deserialize(&mut de)?, value);
    Ok(())
}

#[test]
fn deferred_arc_skips_fetch_then_preserves_identity() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let keys = serialize_shared_arc_items(&storage, 2)?;
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let data = storage.fetch_data_blocking(&keys[0])?;
    let mut de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    let before = storage.fetch_count.load(Ordering::SeqCst);
    let deferred = crate::DeferredValue::<Arc<Vec<u8>>>::deserialize_arc(&mut de, true)?;
    assert!(!deferred.is_resolved());
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), before);
    let loaded = deferred.read()?;
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), before + 1);
    assert_eq!(loaded.as_slice(), &[0xAB; 1000]);

    let mut eager_de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut eager_de)?;
    let eager = crate::DeferredValue::<Arc<Vec<u8>>>::deserialize_arc(&mut eager_de, false)?;
    assert!(eager.is_resolved());
    assert!(Arc::ptr_eq(loaded, eager.read()?));
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), before + 1);
    Ok(())
}

#[test]
fn deferred_arc_metadata_skips_fetch_then_preserves_identity() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let keys = serialize_shared_arc_items(&storage, 1)?;
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let data = storage.fetch_data_blocking(&keys[0])?;
    let mut de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    let before = storage.fetch_count.load(Ordering::SeqCst);
    let deferred = crate::DeferredValue::<Arc<Vec<u8>>, u8>::deserialize_arc_with_metadata(
        &mut de, true, 0xAB,
    )?;
    assert_eq!(*deferred.metadata_or_else(|v| &v[0]), 0xAB);
    assert!(!deferred.is_resolved());
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), before);
    let loaded = deferred.read()?;
    assert_eq!(loaded.as_slice(), &[0xAB; 1000]);
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), before + 1);

    let mut eager_de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut eager_de)?;
    let eager = crate::DeferredValue::<Arc<Vec<u8>>, u8>::deserialize_arc_with_metadata(
        &mut eager_de,
        false,
        0xAB,
    )?;
    assert!(eager.is_resolved());
    assert_eq!(*eager.metadata_or_else(|v| &v[0]), 0xAB);
    assert!(Arc::ptr_eq(loaded, eager.read()?));
    assert_eq!(storage.fetch_count.load(Ordering::SeqCst), before + 1);
    Ok(())
}

#[test]
fn unread_native_field_serializes_to_another_storage() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let keys = serialize_shared_arc_items(&storage, 1)?;
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let data = storage.fetch_data_blocking(&keys[0])?;
    let mut de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    let deferred = crate::DeferredValue::<Arc<Vec<u8>>>::deserialize_arc(&mut de, true)?;

    let destination = InMemoryPagableStorage::new();
    let target = destination.handle();
    let context = target.storage_context();
    let mut serializer = SerializerForPaging::new(context);
    deferred.pagable_serialize(&mut serializer)?;
    let (bytes, arcs) = serializer.finish();
    let key = target
        .page_out_item(bytes, arcs, &ArcSerCache::new(), context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => anyhow::anyhow!("unexpected prior page-out failure"),
        })?;
    drop(de);
    drop(handle);
    drop(storage);
    drop(mem);
    drop(deferred);
    target.arc_cache().clear();
    let target_handle = PagableStorageHandle::new(target.clone());
    let data = target.fetch_data_blocking(&key)?;
    let mut de = target_handle.root_deserializer(key, &data);
    let loaded = crate::DeferredValue::<Arc<Vec<u8>>>::deserialize_arc(&mut de, true)?;
    assert_eq!(loaded.read()?.as_slice(), &[0xAB; 1000]);
    Ok(())
}

#[test]
fn unread_native_field_does_not_keep_storage_alive() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let keys = serialize_shared_arc_items(&storage, 1)?;
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let data = storage.fetch_data_blocking(&keys[0])?;
    let mut de = handle.root_deserializer(keys[0], &data);
    let _: u8 = crate::PagableDeserialize::pagable_deserialize(&mut de)?;
    let deferred = crate::DeferredValue::<Arc<Vec<u8>>>::deserialize_arc(&mut de, true)?;
    let weak = Arc::downgrade(&storage);
    drop(de);
    drop(handle);
    drop(storage);
    assert!(weak.upgrade().is_none());
    assert!(deferred.read().is_err());
    Ok(())
}

/// A lazily bound arc is created once per key and shared with every later
/// binder, and an arc already cached under the key wins over `make`.
#[test]
fn bind_arc_by_key_lazily_shares_one_allocation() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let key = DataKey::testing_new(7);
    let made = Arc::new(AtomicUsize::new(0));

    let make = |made: &Arc<AtomicUsize>| {
        let made = made.dupe();
        move || {
            made.fetch_add(1, Ordering::SeqCst);
            Box::new(Arc::new(vec![1u8, 2, 3])) as Box<dyn ArcEraseDyn>
        }
    };
    let first = handle.bind_arc_by_key_lazily(key, TypeId::of::<Arc<Vec<u8>>>(), make(&made));
    let second = handle.bind_arc_by_key_lazily(key, TypeId::of::<Arc<Vec<u8>>>(), make(&made));
    assert_eq!(
        made.load(Ordering::SeqCst),
        1,
        "the second binder reuses the first arc"
    );
    assert_eq!(
        first.identity(),
        second.identity(),
        "one allocation under the key"
    );
    assert_eq!(
        storage.fetch_count.load(Ordering::SeqCst),
        0,
        "binding lazily never fetches"
    );
    Ok(())
}

/// A lazily bound arc carries its row's key before any lookup can see it.
#[test]
fn lazily_bound_arc_is_keyed_before_it_is_visible() {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let key = DataKey::testing_new(11);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();

    let bound = handle.bind_arc_by_key_lazily(key, type_id, || {
        Box::new(PartialPagableArc::new(ResidentArcValue(1))) as Box<dyn ArcEraseDyn>
    });

    assert_eq!(bound.data_key(), Some(key));
    assert!(
        !storage.keyed_while_visible.load(Ordering::SeqCst),
        "the arc was published to the cache before it was keyed"
    );
}

/// Parallel `page_out_item` calls sharing the same `finished` map must
/// not serialize the same arc more than once.
#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    all(miri, not(target_os = "linux")),
    ignore = "tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll"
)]
async fn page_out_does_not_duplicate_arc_serialization() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));

    let shared_arc: Arc<Vec<u8>> = Arc::new(vec![0xAB; 1000]);
    let finished = Arc::new(ArcSerCache::new());

    let num_items = 100usize;
    let handles: Vec<_> = (0..num_items)
        .map(|i| {
            let storage = storage.clone();
            let shared_arc = shared_arc.clone();
            let finished = finished.clone();
            tokio::spawn(async move {
                let storage_context = storage.storage_context();
                let mut ser = SerializerForPaging::new(storage_context);
                (i as u8).pagable_serialize(&mut ser)?;
                shared_arc.pagable_serialize(&mut ser)?;
                let (data, arcs) = ser.finish();
                storage
                    .page_out_item(data, arcs, &finished, storage_context)
                    .map_err(|e| match e {
                        PageOutError::Failed(e) => e,
                        PageOutError::AlreadyFailed => {
                            panic!("unexpected AlreadyFailed")
                        }
                    })?;
                Ok::<_, anyhow::Error>(())
            })
        })
        .collect();

    for h in handles {
        h.await??;
    }
    storage.flush()?;

    let total = storage.store_count.load(Ordering::SeqCst);
    assert!(
        total <= num_items + 1,
        "expected at most {} store_data calls, got {}",
        num_items + 1,
        total,
    );
    Ok(())
}

/// A page-out that observed no key must reuse one recorded by a concurrent
/// page-out before serialization begins.
#[test]
fn page_out_reuses_data_key_recorded_during_serialization() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let arc = PausedDataKeyLookupArc::new(ResidentArcValue(42));

    let racing_storage = storage.dupe();
    let racing_arc = arc.clone();
    let racing_page_out =
        std::thread::spawn(move || page_out_paused_lookup_arc(&racing_storage, &racing_arc));

    arc.wait_for_paused_lookup();
    let winning_page_out = page_out_paused_lookup_arc(&storage, &arc);
    arc.release_paused_lookup();

    let racing_page_out = racing_page_out
        .join()
        .map_err(|_| anyhow::anyhow!("racing page-out thread panicked"))?;
    assert_eq!(
        winning_page_out?, racing_page_out?,
        "both page-outs should reuse the same stored Arc representation",
    );
    Ok(())
}

#[test]
fn page_out_reuses_data_key_recorded_during_deserialization() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);

    let original = PagableArc::new(42u8, handle.dupe());
    let first_finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    1u8.pagable_serialize(&mut ser)?;
    original.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();
    let first_parent_key = storage
        .page_out_item(data, arcs, &first_finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    storage.flush()?;
    drop(original);

    let first_parent = storage.fetch_data_blocking(&first_parent_key)?;
    let child_key = first_parent.arcs[0];
    let mut deser = handle.root_deserializer(first_parent_key, &first_parent);
    assert_eq!(u8::pagable_deserialize(&mut deser)?, 1);
    let restored = PagableArc::<u8>::pagable_deserialize(&mut deser)?;
    assert_eq!(restored.get_data_key(), Some(child_key));

    let stores_before = storage.store_count.load(Ordering::SeqCst);
    let second_finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    2u8.pagable_serialize(&mut ser)?;
    restored.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();
    let second_parent_key = storage
        .page_out_item(data, arcs, &second_finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    storage.flush()?;

    assert_eq!(
        storage.store_count.load(Ordering::SeqCst) - stores_before,
        1,
        "only the new parent should be written; its stored child should be reused",
    );
    let second_parent = storage.fetch_data_blocking(&second_parent_key)?;
    assert_eq!(second_parent.arcs.as_slice(), &[child_key]);
    Ok(())
}

fn page_in_partial_arc(
    cache: &DeserializedArcCache,
    key: DataKey,
    arc: &PartialPagableArc<ResidentArcValue>,
) -> Box<dyn ArcEraseDyn> {
    cache
        .get_or_page_in(
            TypeId::of::<PartialPagableArc<ResidentArcValue>>(),
            key,
            || Ok::<_, anyhow::Error>(Box::new(arc.dupe())),
        )
        .expect("page-in closure is infallible")
}

fn downcast_partial_arc(arc: &dyn ArcEraseDyn) -> &PartialPagableArc<ResidentArcValue> {
    arc.as_arc_any()
        .downcast_ref::<PartialPagableArc<ResidentArcValue>>()
        .expect("cached Arc type should match")
}

#[test]
fn live_resident_arc_takes_precedence_over_deserialized_arc() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();

    let deserialized = PartialPagableArc::new(ResidentArcValue(1));
    ArcErase::set_data_key(&deserialized, key);
    let bound = page_in_partial_arc(&cache, key, &deserialized);
    assert!(PartialPagableArc::ptr_eq(
        &deserialized,
        downcast_partial_arc(&*bound)
    ));

    let resident = PartialPagableArc::new(ResidentArcValue(1));
    ArcErase::set_data_key(&resident, key);
    cache.register_resident(key, &resident);

    let selected = cache.get(&type_id, &key).expect("cached Arc should exist");
    let selected = downcast_partial_arc(&*selected);
    assert!(
        PartialPagableArc::ptr_eq(&resident, selected),
        "the live resident allocation should supersede the older deserialized allocation",
    );
    assert!(!PartialPagableArc::ptr_eq(&deserialized, selected));
}

#[test]
fn page_in_keeps_a_resident_arc_registered_while_it_ran() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();

    let resident = PartialPagableArc::new(ResidentArcValue(1));
    ArcErase::set_data_key(&resident, key);
    let fresh = PartialPagableArc::new(ResidentArcValue(1));
    let bound = cache
        .get_or_page_in(type_id, key, || {
            // A page-out registers the resident allocation under the key while
            // this page-in is still deserializing its own.
            cache.register_resident(key, &resident);
            Ok::<_, anyhow::Error>(Box::new(fresh.dupe()))
        })
        .expect("page-in closure is infallible");
    assert!(
        PartialPagableArc::ptr_eq(&resident, downcast_partial_arc(&*bound)),
        "page-in hands out the allocation a resident graph already holds",
    );
    drop(bound);
    let again = cache.get(&type_id, &key).expect("cached Arc should exist");
    assert!(PartialPagableArc::ptr_eq(
        &resident,
        downcast_partial_arc(&*again)
    ));
    assert!(!PartialPagableArc::ptr_eq(
        &fresh,
        downcast_partial_arc(&*again)
    ));
}

#[test]
fn dropped_arc_is_paged_in_again_and_its_entry_evicted() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();

    let first = PartialPagableArc::new(ResidentArcValue(1));
    drop(page_in_partial_arc(&cache, key, &first));
    drop(first);
    assert!(
        cache.get(&type_id, &key).is_none(),
        "the cache must not keep the allocation alive",
    );

    let second = PartialPagableArc::new(ResidentArcValue(2));
    let bound = page_in_partial_arc(&cache, key, &second);
    assert!(PartialPagableArc::ptr_eq(
        &second,
        downcast_partial_arc(&*bound)
    ));

    assert_eq!(cache.len(), 1);
    cache.evict_expired();
    assert_eq!(cache.len(), 1, "a live binding survives eviction");
    drop((bound, second));
    cache.evict_expired();
    assert_eq!(cache.len(), 0);
}

#[test]
fn arc_without_weak_form_is_evicted_once_the_cache_is_its_last_holder() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<triomphe::Arc<u8>>();

    let arc = triomphe::Arc::new(7u8);
    drop(
        cache
            .get_or_page_in(type_id, key, || {
                Ok::<_, anyhow::Error>(Box::new(arc.clone()))
            })
            .expect("page-in closure is infallible"),
    );
    cache.evict_expired();
    assert!(
        cache.get(&type_id, &key).is_some(),
        "an allocation with a live referrer stays bound",
    );

    drop(arc);
    cache.evict_expired();
    assert!(cache.get(&type_id, &key).is_none());
    assert_eq!(cache.len(), 0);
}

#[test]
fn evicting_an_arc_evicts_the_arcs_only_its_value_held() {
    let cache = DeserializedArcCache::new();
    let inner = triomphe::Arc::new(7u8);
    let outer = triomphe::Arc::new(inner.clone());
    let bind = |type_id: TypeId, key: u128, arc: Box<dyn ArcEraseDyn>| {
        drop(
            cache
                .get_or_page_in(type_id, DataKey::testing_new(key), || {
                    Ok::<_, anyhow::Error>(arc)
                })
                .expect("page-in closure is infallible"),
        );
    };
    bind(
        TypeId::of::<triomphe::Arc<u8>>(),
        1,
        Box::new(inner.clone()),
    );
    bind(
        TypeId::of::<triomphe::Arc<triomphe::Arc<u8>>>(),
        2,
        Box::new(outer.clone()),
    );
    drop((inner, outer));

    // The inner arc is held by the outer value until the outer binding drops.
    cache.evict_expired();
    assert_eq!(cache.len(), 0);
}

#[derive(pagable::PagableUnsupported)]
struct NativeCacheChainNode {
    child: Option<triomphe::Arc<NativeCacheChainNode>>,
    drops: Arc<AtomicUsize>,
}

impl Drop for NativeCacheChainNode {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn cached_native_chain(
    cache: &DeserializedArcCache,
    drops: &Arc<AtomicUsize>,
    len: usize,
) -> triomphe::Arc<NativeCacheChainNode> {
    (0..len)
        .fold(None, |child, index| {
            let arc = triomphe::Arc::new(NativeCacheChainNode {
                child,
                drops: drops.dupe(),
            });
            drop(
                cache
                    .get_or_page_in(
                        TypeId::of::<triomphe::Arc<NativeCacheChainNode>>(),
                        DataKey::testing_new(index as u128 + 1),
                        || Ok::<_, anyhow::Error>(Box::new(arc.clone())),
                    )
                    .expect("page-in closure is infallible"),
            );
            Some(arc)
        })
        .expect("the test chain is nonempty")
}

#[test]
fn a_native_cache_chain_is_collected_in_one_pass() {
    const NODES: usize = 500;
    let cache = DeserializedArcCache::new();
    let drops = Arc::new(AtomicUsize::new(0));
    drop(cached_native_chain(&cache, &drops, NODES));

    let stats = cache.evict_expired();
    assert_eq!(stats.passes, 2);
    assert_eq!(stats.entries_examined, NODES);
    assert_eq!(stats.removed, NODES);
    assert_eq!(drops.load(Ordering::SeqCst), NODES);
}

#[test]
fn out_of_order_bindings_are_collected_by_fresh_fallback_passes() {
    const NODES: usize = 8;
    let cache = DeserializedArcCache::new();
    let drops = Arc::new(AtomicUsize::new(0));
    let root = cached_native_chain(&cache, &drops, NODES);

    // Rebind parents before children, deliberately reversing the sweep hint.
    let mut node = Some(&root);
    let mut index = NODES;
    while let Some(arc) = node {
        cache.bind(
            TypeId::of::<triomphe::Arc<NativeCacheChainNode>>(),
            DataKey::testing_new(index as u128),
            arc,
        );
        index -= 1;
        node = arc.child.as_ref();
    }
    assert_eq!(index, 0);
    drop(root);

    let stats = cache.evict_expired();
    assert_eq!(stats.passes, NODES + 1);
    assert_eq!(stats.entries_examined, NODES * (NODES + 1) / 2);
    assert_eq!(stats.removed, NODES);
    assert_eq!(drops.load(Ordering::SeqCst), NODES);
}

#[test]
fn eviction_stats_count_successful_removals_not_stale_candidates() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();
    let first = PartialPagableArc::new(ResidentArcValue(1));
    drop(page_in_partial_arc(&cache, key, &first));
    drop(first);

    let expired: Vec<_> = cache
        .map
        .iter()
        .filter(|entry| entry.value().expired())
        .map(|entry| *entry.key())
        .collect();
    assert_eq!(expired, vec![(type_id, key)]);

    // A page-in between the scan and removal binds a live allocation to the
    // same key. Replay that interleaving without depending on thread scheduling.
    let replacement = PartialPagableArc::new(ResidentArcValue(2));
    drop(page_in_partial_arc(&cache, key, &replacement));
    assert_eq!(cache.remove_expired(expired), 0);
    let bound = cache.get(&type_id, &key).expect("live rebind survives");
    assert!(PartialPagableArc::ptr_eq(
        &replacement,
        downcast_partial_arc(&*bound),
    ));
    drop((bound, replacement));
    let stats = cache.evict_expired();
    assert_eq!(stats.passes, 2);
    assert_eq!(stats.entries_examined, 1);
    assert_eq!(stats.removed, 1);
}

#[test]
fn eviction_preserves_a_page_in_gate_replacing_an_expired_binding() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();
    let arc = PartialPagableArc::new(ResidentArcValue(1));
    drop(page_in_partial_arc(&cache, key, &arc));
    drop(arc);
    assert!(cache.map.get(&(type_id, key)).unwrap().expired());

    let gate = Arc::new(Mutex::new(()));
    let previous = cache
        .map
        .insert((type_id, key), Slot::PagingIn(gate.dupe()));
    drop(previous);
    assert_eq!(cache.remove_expired(vec![(type_id, key)]), 0);
    assert!(matches!(
        cache.map.get(&(type_id, key)).unwrap().value(),
        Slot::PagingIn(bound) if Arc::ptr_eq(bound, &gate),
    ));
    assert_eq!(cache.evict_expired().removed, 0);
}

#[derive(pagable::PagableUnsupported)]
struct CacheDropAction {
    cache: Weak<DeserializedArcCache>,
    own_key: DataKey,
    action: Box<dyn Fn(&DeserializedArcCache) + Send + Sync>,
}

impl Drop for CacheDropAction {
    fn drop(&mut self) {
        let cache = self.cache.upgrade().expect("the test holds the cache");
        let own_key = (TypeId::of::<triomphe::Arc<Self>>(), self.own_key);
        // A nonblocking probe fails promptly instead of deadlocking if the
        // removal still holds this value's shard lock during its destructor.
        assert!(!matches!(cache.map.try_get(&own_key), TryResult::Locked));
        assert!(matches!(cache.map.entry(own_key), Entry::Vacant(_)));
        (self.action)(&cache);
    }
}

fn bind_cache_drop_action(
    cache: &Arc<DeserializedArcCache>,
    key: DataKey,
    action: impl Fn(&DeserializedArcCache) + Send + Sync + 'static,
) {
    let arc = triomphe::Arc::new(CacheDropAction {
        cache: Arc::downgrade(cache),
        own_key: key,
        action: Box::new(action),
    });
    cache.bind(TypeId::of::<triomphe::Arc<CacheDropAction>>(), key, &arc);
}

#[test]
fn drop_callback_rebinds_a_snapshot_key_without_holding_its_shard_lock() {
    let cache = Arc::new(DeserializedArcCache::new());
    let target_key = DataKey::testing_new(1);
    let type_id = TypeId::of::<PartialPagableArc<ResidentArcValue>>();
    let stale = PartialPagableArc::new(ResidentArcValue(1));
    drop(page_in_partial_arc(&cache, target_key, &stale));
    drop(stale);

    let replacement = PartialPagableArc::new(ResidentArcValue(2));
    ArcErase::set_data_key(&replacement, target_key);
    let callbacks = Arc::new(AtomicUsize::new(0));
    bind_cache_drop_action(&cache, DataKey::testing_new(2), {
        let replacement = replacement.dupe();
        let callbacks = callbacks.dupe();
        move |cache| {
            cache.register_resident(target_key, &replacement);
            callbacks.fetch_add(1, Ordering::SeqCst);
        }
    });

    let stats = cache.evict_expired();
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(stats.passes, 2);
    assert_eq!(
        stats.entries_examined, 3,
        "two initial visits plus one live survivor"
    );
    assert_eq!(stats.removed, 1);
    let bound = cache
        .get(&type_id, &target_key)
        .expect("the rebind survives");
    assert!(PartialPagableArc::ptr_eq(
        &replacement,
        downcast_partial_arc(&*bound),
    ));
    drop((bound, replacement));
    assert_eq!(cache.evict_expired().removed, 1);
}

#[test]
fn a_drop_callback_new_binding_is_visited_by_a_fresh_snapshot() {
    let cache = Arc::new(DeserializedArcCache::new());
    let callbacks = Arc::new(AtomicUsize::new(0));
    bind_cache_drop_action(&cache, DataKey::testing_new(1), {
        let callbacks = callbacks.dupe();
        move |cache| {
            let arc = triomphe::Arc::new(7u8);
            cache.bind(
                TypeId::of::<triomphe::Arc<u8>>(),
                DataKey::testing_new(2),
                &arc,
            );
            callbacks.fetch_add(1, Ordering::SeqCst);
        }
    });

    let stats = cache.evict_expired();
    assert_eq!(callbacks.load(Ordering::SeqCst), 1);
    assert_eq!(stats.passes, 3);
    assert_eq!(stats.entries_examined, 2);
    assert_eq!(stats.removed, 2);
}

#[test]
fn an_expired_rebind_is_eligible_despite_its_new_order() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let stale = PartialPagableArc::new(ResidentArcValue(1));
    drop(page_in_partial_arc(&cache, key, &stale));
    drop(stale);
    let bindings = cache.bindings_in_eviction_order();
    assert_eq!(bindings.len(), 1);

    let replacement = PartialPagableArc::new(ResidentArcValue(2));
    drop(page_in_partial_arc(&cache, key, &replacement));
    drop(replacement);
    assert_eq!(
        cache.remove_expired(bindings.into_iter().map(|(_, key)| key)),
        1
    );
    assert_eq!(cache.len(), 0);
}

#[test]
fn a_failed_page_in_leaves_the_key_to_the_next_caller() {
    let cache = DeserializedArcCache::new();
    let key = DataKey::testing_new(1);
    let type_id = TypeId::of::<triomphe::Arc<u8>>();

    let failed = cache.get_or_page_in(type_id, key, || {
        Err::<Box<dyn ArcEraseDyn>, _>(anyhow::anyhow!("row unreadable"))
    });
    assert!(failed.is_err());
    assert_eq!(cache.len(), 0, "a failed page-in leaves no gate behind");

    let arc = triomphe::Arc::new(7u8);
    let bound = cache
        .get_or_page_in(type_id, key, || {
            Ok::<_, anyhow::Error>(Box::new(arc.clone()))
        })
        .expect("page-in closure is infallible");
    assert_eq!(ArcEraseDyn::identity(&*bound), ArcErase::identity(&arc));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "parks threads, and `parking_lot_core`'s wait queue casts integers to pointers"
)]
fn concurrent_page_ins_of_one_key_deserialize_once() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let original = PartialPagableArc::new(ResidentArcValue(42));

    let finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    original.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();
    let parent_key = storage
        .page_out_item(data, arcs, &finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    drop(original);

    let parent = storage.fetch_data_blocking(&parent_key)?;
    let fetches_before = storage.fetch_count.load(Ordering::SeqCst);
    const THREADS: usize = 8;
    let barrier = Barrier::new(THREADS);
    let restored: Vec<PartialPagableArc<ResidentArcValue>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let mut deser = handle.root_deserializer(parent_key, &parent);
                    deserialize_resident_partial_arc(&mut deser)
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("page-in thread panicked"))
            .collect::<crate::Result<Vec<_>>>()
    })?;

    assert_eq!(
        storage.fetch_count.load(Ordering::SeqCst) - fetches_before,
        1,
        "one page-in should fetch the child; the rest share its allocation",
    );
    assert!(
        restored
            .iter()
            .all(|arc| PartialPagableArc::ptr_eq(arc, &restored[0]))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    all(miri, not(target_os = "linux")),
    ignore = "tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll"
)]
async fn page_in_reuses_pagable_arc_identity_without_pinning_it() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);

    let original = PagableArc::new(42u8, handle.dupe());
    let original_identity = ArcErase::identity(&original);
    let finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    original.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();
    let parent_key = storage
        .page_out_item(data, arcs, &finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    storage.flush()?;
    drop(original);

    let parent = storage.fetch_data_blocking(&parent_key)?;
    let mut deser = handle.root_deserializer(parent_key, &parent);
    let restored = PagableArc::<u8>::pagable_deserialize(&mut deser)?;
    assert_eq!(
        ArcErase::identity(&restored),
        original_identity,
        "page-in should reuse the allocation registered during page-out",
    );
    assert!(!restored.is_pinned());
    assert!(
        restored.is_paged_out(),
        "the cache entry must not keep the value resident",
    );
    assert_eq!(*restored.pin().await?, 42);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg_attr(
    all(miri, not(target_os = "linux")),
    ignore = "tokio's IO driver is epoll, kqueue or IOCP by OS, and Miri models only epoll"
)]
async fn page_in_of_pinned_pagable_arc_does_not_pin_it_through_the_cache() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);

    let original = PinnedPagableArc::new(42u8, handle.dupe());
    let finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    original.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();
    let parent_key = storage
        .page_out_item(data, arcs, &finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;
    storage.flush()?;
    drop(original);

    let parent = storage.fetch_data_blocking(&parent_key)?;
    let page_in = || -> anyhow::Result<PinnedPagableArc<u8>> {
        let mut deser = handle.root_deserializer(parent_key, &parent);
        PinnedPagableArc::<u8>::pagable_deserialize(&mut deser)
    };
    let restored = page_in()?;
    let again = page_in()?;
    assert!(
        PinnedPagableArc::ptr_eq(&restored, &again),
        "page-in should reuse the resident allocation",
    );
    drop(again);

    let unpinned = restored.into_pagable();
    unpinned.unpin();
    assert_eq!(
        unpinned.pinned_count(),
        0,
        "the cache entry must not keep the value pinned",
    );
    assert_eq!(*page_in()?, 42);
    Ok(())
}

#[test]
fn page_in_reuses_arc_still_resident_after_page_out() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));
    let handle = PagableStorageHandle::new(storage.dupe() as Arc<dyn PagableStorage>);
    let original = PartialPagableArc::new(ReuseProbe(42));

    let finished = ArcSerCache::new();
    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    original.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();
    let parent_key = storage
        .page_out_item(data, arcs, &finished, storage_context)
        .map_err(|e| match e {
            PageOutError::Failed(e) => e,
            PageOutError::AlreadyFailed => panic!("unexpected AlreadyFailed"),
        })?;

    REUSE_PROBE_DESERIALIZATIONS.store(0, Ordering::SeqCst);
    let parent = storage.fetch_data_blocking(&parent_key)?;
    let child_key = parent.arcs[0];
    let mut deser = handle.root_deserializer(parent_key, &parent);
    let restored: PartialPagableArc<ReuseProbe> = deserialize_partial_arc(&mut deser)?;

    assert!(
        PartialPagableArc::ptr_eq(&original, &restored),
        "page-in should reuse the allocation retained across page-out",
    );
    assert_eq!(restored.0, 42);
    assert_eq!(REUSE_PROBE_DESERIALIZATIONS.load(Ordering::SeqCst), 0);

    drop(restored);
    drop(original);
    assert!(
        storage
            .arc_cache()
            .get(&TypeId::of::<PartialPagableArc<ReuseProbe>>(), &child_key,)
            .is_none(),
        "the resident registry must not keep the allocation alive",
    );

    let mut deser = handle.root_deserializer(parent_key, &parent);
    let restored: PartialPagableArc<ReuseProbe> = deserialize_partial_arc(&mut deser)?;
    assert_eq!(restored.0, 42);
    assert_eq!(REUSE_PROBE_DESERIALIZATIONS.load(Ordering::SeqCst), 1);
    Ok(())
}

struct FailingSer;

impl PagableSerialize for FailingSer {
    fn pagable_serialize(&self, _ser: &mut dyn PagableSerializer) -> crate::Result<()> {
        Err(anyhow::anyhow!("intentional serialization failure"))
    }
}

impl<'de> PagableDeserialize<'de> for FailingSer {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        _deserializer: &mut D,
    ) -> crate::Result<Self> {
        unreachable!()
    }
}

/// `page_out_item` must return `Failed` with the original error when a
/// nested arc fails to serialize.
#[test]
fn page_out_propagates_nested_arc_serialization_failure() -> anyhow::Result<()> {
    let mem = InMemoryPagableStorage::new();
    let storage = Arc::new(CountingStorage::new(mem.handle()));

    let failing_arc: Arc<FailingSer> = Arc::new(FailingSer);
    let finished = ArcSerCache::new();

    let storage_context = storage.storage_context();
    let mut ser = SerializerForPaging::new(storage_context);
    42u8.pagable_serialize(&mut ser)?;
    failing_arc.pagable_serialize(&mut ser)?;
    let (data, arcs) = ser.finish();

    let result = storage.page_out_item(data, arcs, &finished, storage_context);
    let err = match result {
        Err(PageOutError::Failed(e)) => e,
        other => panic!("expected Failed, got {:?}", other.is_ok()),
    };
    assert!(
        format!("{:#}", err).contains("intentional serialization failure"),
        "should contain the original error message, got: {:#}",
        err,
    );
    Ok(())
}

#[derive(pagable::PagableUnsupported)]
struct Unpagable;

#[test]
fn unsupported_derive_fails_instead_of_panicking() {
    let mut serializer = crate::testing::TestingSerializer::new();
    let error = Unpagable
        .pagable_serialize(&mut serializer)
        .expect_err("an unsupported type refuses to page out");
    assert!(
        format!("{error:#}").contains("`Unpagable` cannot be paged out"),
        "{error:#}"
    );
    let mut deserializer = crate::testing::TestingDeserializer::new(&[]);
    assert!(Unpagable::pagable_deserialize(&mut deserializer).is_err());
}

#[test]
fn frontier_covers_tickets_whose_shards_have_passed_them() {
    let a = WriteTicket::new(0, 5);
    let b = WriteTicket::new(1, 7);
    let both = a.join(b);
    let frontier = CommitFrontier::new(vec![6, 7]);
    assert!(frontier.covers(&a));
    assert!(!frontier.covers(&b), "b is the first row not committed");
    assert!(!frontier.covers(&both));
    assert!(CommitFrontier::new(vec![8, 8]).covers(&both));
    assert!(CommitFrontier::everything().covers(&both));
    assert!(CommitFrontier::new(vec![1]).covers(&WriteTicket::DURABLE));
}
