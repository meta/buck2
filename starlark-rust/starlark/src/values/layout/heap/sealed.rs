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

//! A sealed frozen heap: the shared allocation behind an [`OwnedFrozen`], its wire format for
//! paging, and the diagnostics for a value that cannot be located.

use std::any::TypeId;
use std::collections::HashSet;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::hash::Hash;
use std::hash::Hasher;
use std::mem;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::ptr;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::Weak;

use allocative::Allocative;
use allocative::FlameGraphBuilder;
use dupe::Dupe;
use once_cell::race::OnceBox;
use pagable::DataKey;
use pagable::PagableCursor;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use pagable::PartialPagableArc;
use pagable::PartialPagableWeak;
use pagable::arc_erase::ArcEraseDyn;
use pagable::arc_erase::WeakErase;
use pagable::storage::handle::PagableStorageHandle;
use rand::RngExt;

pub(crate) use self::heap_key_index::load_and_bind_heap_by_id;
pub(crate) use self::heap_key_index::register_heap_key_index;
use crate::eval::runtime::profile::instant::ProfilerInstant;
use crate::pagable::error::PagableError;
use crate::pagable::heap_ref_id::HeapRefId;
use crate::pagable::starlark_deserialize_context::HeapDeserializationState;
use crate::pagable::starlark_deserialize_context::StarlarkDeserScope;
use crate::pagable::starlark_deserialize_context::StarlarkDeserializerImpl;
use crate::pagable::starlark_serialize::StarlarkSerializeContext;
use crate::pagable::starlark_serialize_context::StarlarkSerState;
use crate::pagable::starlark_serialize_context::StarlarkSerializerImpl;
use crate::pagable::static_value::StaticHeapId;
use crate::pagable::static_value::get_static_heap_by_id;
use crate::pagable::static_value::get_static_heap_id;
use crate::values::OwnedFrozen;
use crate::values::layout::heap::allocator::alloc::allocator::ChunkAllocator;
use crate::values::layout::heap::arena::Arena;
use crate::values::layout::heap::arena::ArenaEntry;
use crate::values::layout::heap::arena::ArenaVisitor;
use crate::values::layout::heap::arena::ChunkInfo;
use crate::values::layout::heap::arena::HeapKind;
use crate::values::layout::heap::name::FrozenHeapName;
use crate::values::layout::heap::profile::by_type::HeapSummary;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::value::Value;

#[cfg(fbcode_build)]
pub(crate) mod heap_key_index {
    use std::any::TypeId;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use dashmap::DashMap;
    use pagable::DataKey;
    use pagable::PartialPagableArc;
    use pagable::StorageState;
    use pagable::arc_erase::ArcEraseDyn;
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::traits::StorageContext;

    use super::FrozenFrozenHeap;
    use super::FrozenHeapArc;
    use super::deserialize_heap_arc_with_recipe;
    use super::downcast_heap_arc;
    use crate::pagable::heap_ref_id::HeapRefId;
    use crate::pagable::starlark_deserialize_context::StarlarkDeserScope;

    /// Where each heap's data lives, keyed by the heap's logical identity.
    ///
    /// A ref list names direct dependencies, and a pointer normally lands in one
    /// of those. A pointer into a heap no ref list read so far names - one
    /// relocated into a shared blob, say - can only be found by identity, and this
    /// is the map that answers it.
    ///
    /// Populated on writes and binds; entries stay in memory for the storage's
    /// lifetime, as the missing-heap fallback requires.
    #[derive(Default)]
    pub(crate) struct StarlarkHeapKeyIndex {
        keys: DashMap<HeapRefId, DataKey>,
        /// Heaps indexed under a second key since the last [`take_heap_rekeys`].
        rekeyed: AtomicUsize,
    }

    impl StorageState for StarlarkHeapKeyIndex {}

    impl StarlarkHeapKeyIndex {
        pub(crate) fn get(&self, heap_id: &HeapRefId) -> Option<DataKey> {
            self.keys.get(heap_id).map(|key| *key)
        }

        /// An id names one heap and one heap serializes under one key, so a
        /// repeated insert carries the same key; one that does not is kept for
        /// [`take_heap_rekeys`].
        pub(crate) fn insert(&self, heap_id: HeapRefId, key: DataKey) {
            if self
                .keys
                .insert(heap_id, key)
                .is_some_and(|previous| previous != key)
            {
                self.rekeyed.fetch_add(1, Ordering::Relaxed);
            }
        }

        #[cfg(test)]
        pub(crate) fn clear(&self) {
            self.keys.clear();
        }
    }

    /// How many heaps this storage indexed under a second data key since the
    /// last call. The resident arc for a heap is registered under the key the
    /// heap was first bound to, so a page-in through a second key cannot find
    /// the live allocation and deserializes a copy of it, which then conflicts
    /// with the original. It should never happen; the caller surfaces it so
    /// that it is seen when it does.
    pub fn take_heap_rekeys(storage: &StorageContext) -> usize {
        storage
            .get::<StarlarkHeapKeyIndex>()
            .map_or(0, |index| index.rekeyed.swap(0, Ordering::Relaxed))
    }

    /// Records a heap's `DataKey` against its [`HeapRefId`] as pagable assigns it.
    fn index_heap_data_key(context: &StorageContext, arc: &dyn ArcEraseDyn, key: DataKey) {
        let Some(heap) = arc
            .as_arc_any()
            .downcast_ref::<PartialPagableArc<FrozenFrozenHeap>>()
        else {
            return;
        };
        let Some(heap_id) = heap.heap_ref_id() else {
            return;
        };
        context
            .get_or_init(StarlarkHeapKeyIndex::default)
            .insert(heap_id, key);
    }

    /// Start indexing heaps by identity on this storage. Idempotent.
    ///
    /// Called when the storage's Starlark serialization state or a page-in's
    /// deserialization scope is created, which every path that assigns a heap
    /// a `DataKey` goes through first, so no heap is written or read unindexed.
    pub(crate) fn register_heap_key_index(context: &StorageContext) {
        context.observe_arc_data_keys(index_heap_data_key);
    }

    /// Load the heap with identity `heap_id`, fetching its data unless the arc
    /// cache holds it, and bind it into `scope`. For a pointer into a heap no
    /// ref list read so far names; the [`StarlarkHeapKeyIndex`] supplies the
    /// key. `None` when the index has no entry for it.
    pub(crate) fn load_and_bind_heap_by_id(
        scope: &StarlarkDeserScope,
        storage: &PagableStorageHandle,
        heap_id: HeapRefId,
    ) -> crate::Result<Option<FrozenHeapArc>> {
        let Some(key) = storage
            .storage_context()
            .get::<StarlarkHeapKeyIndex>()
            .and_then(|index| index.get(&heap_id))
        else {
            return Ok(None);
        };
        let arc_box = storage
            .deserialize_arc_by_key(
                key,
                TypeId::of::<PartialPagableArc<FrozenFrozenHeap>>(),
                deserialize_heap_arc_with_recipe,
            )
            .map_err(crate::Error::new_other)?;
        let heap = downcast_heap_arc(&*arc_box)?;
        heap.register_heap_graph_in_deser_scope(scope)?;
        Ok(Some(heap))
    }
}

// The published pagable crate does not support heap-key observers or lookups yet.
#[cfg(not(fbcode_build))]
pub(crate) mod heap_key_index {
    use pagable::storage::handle::PagableStorageHandle;
    use pagable::traits::StorageContext;

    use super::FrozenHeapArc;
    use crate::pagable::heap_ref_id::HeapRefId;
    use crate::pagable::starlark_deserialize_context::StarlarkDeserScope;

    pub(crate) fn register_heap_key_index(_context: &StorageContext) {}

    /// No heap-key index, so never any heap indexed under a second key.
    pub fn take_heap_rekeys(_storage: &StorageContext) -> usize {
        0
    }

    pub(crate) fn load_and_bind_heap_by_id(
        _scope: &StarlarkDeserScope,
        _storage: &PagableStorageHandle,
        _heap_id: HeapRefId,
    ) -> crate::Result<Option<FrozenHeapArc>> {
        Ok(None)
    }
}

/// Identifies one sealing of a heap, see `FrozenFrozenHeap::serialization_nonce`.
///
/// A `u128` split into low and high words: a `u128` field would make
/// `FrozenFrozenHeap` 16-byte aligned. Serialized as the `u128`.
#[derive(Debug, Clone, Copy, Allocative)]
pub(crate) struct HeapSerializationNonce([u64; 2]);

impl HeapSerializationNonce {
    fn random() -> Self {
        Self::from_u128(rand::rng().random())
    }

    fn from_u128(nonce: u128) -> Self {
        Self([nonce as u64, (nonce >> 64) as u64])
    }

    fn to_u128(self) -> u128 {
        u128::from(self.0[0]) | (u128::from(self.0[1]) << 64)
    }

    pub(crate) fn to_le_bytes(self) -> [u8; 16] {
        self.to_u128().to_le_bytes()
    }
}

impl PagableSerialize for HeapSerializationNonce {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.to_u128().pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for HeapSerializationNonce {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        u128::pagable_deserialize(deserializer).map(Self::from_u128)
    }
}

/// An `Option<usize>` in one word: `Some(n)` is stored as `n + 1` and `None` as zero. A heap's
/// byte count is at most `isize::MAX`, so `n + 1` does not overflow.
#[derive(Clone, Copy, Allocative)]
struct PeakAllocatedBytes(Option<NonZeroUsize>);

impl PeakAllocatedBytes {
    fn new(bytes: Option<usize>) -> Self {
        Self(bytes.map(|bytes| {
            NonZeroUsize::new(bytes.wrapping_add(1))
                .expect("a heap's byte count should be below `usize::MAX`")
        }))
    }

    fn get(self) -> Option<usize> {
        self.0.map(|stored| stored.get() - 1)
    }
}

/// Links that only a heap taking part in a page-out or page-in acquires.
#[derive(Default)]
struct HeapPagingLinks {
    /// Heaps retained beyond the ref list: targets of pointers from this heap's
    /// values that were bound by identity rather than through a ref list, and
    /// so are held here by the heap that uses them.
    retained: Vec<OwnedFrozen<()>>,
    /// Live serialization states this heap is registered with; dropping the
    /// heap unregisters its chunks from each. Production paging creates at
    /// most one `StarlarkSerState` per process, but in-process test runners
    /// (plain `cargo test` runs every test of a binary in one process, each
    /// test with its own state) can register a shared heap with several live
    /// states at once. `register_ser_state` prunes dead entries so that a heap
    /// outliving many states (e.g. a static heap) does not accumulate them.
    ser_states: Vec<Weak<StarlarkSerState>>,
}

#[allow(unused)] // Allocative impls are considered unused
fn visit_paging_links(
    links: &OnceBox<Mutex<HeapPagingLinks>>,
    visitor: &mut allocative::Visitor<'_>,
) {
    // Counts the box, not the vectors' buffers or what they point to.
    let mut visitor = visitor.enter_self_sized::<OnceBox<Mutex<HeapPagingLinks>>>();
    if links.get().is_some() {
        let mut visitor = visitor.enter_unique(
            allocative::Key::new("ptr"),
            mem::size_of::<*const Mutex<HeapPagingLinks>>(),
        );
        visitor.visit_simple(
            allocative::Key::new("HeapPagingLinks"),
            mem::size_of::<Mutex<HeapPagingLinks>>(),
        );
        visitor.exit();
    }
    visitor.exit();
}

/// A sealed frozen heap: no longer allocated on, and shared between threads through
/// [`FrozenHeapArc`].
#[derive(Allocative)]
#[allow(clippy::non_send_fields_in_send_ty)]
struct FrozenFrozenHeap {
    // Keeps content-identical heap incarnations in distinct cache entries while Buck2 still has
    // load-bearing frozen-value pointer identity. Remove this once distinct equal-content
    // allocations are interchangeable; the nonce prevents independent serializations from
    // sharing a `DataKey`.
    #[allocative(skip)]
    serialization_nonce: HeapSerializationNonce,
    arena: Arena<ChunkAllocator>,
    /// Unset for a heap restored from storage whose header is not loaded:
    /// its dependencies are unknown until something resolves into it.
    refs: LazyHeapRefs,
    // TODO(nero): remove Option here, make it required.
    #[allocative(skip)] // We don't really expect it to be big
    name: Option<FrozenHeapName>,
    #[allocative(visit = visit_paging_links)]
    paging_links: OnceBox<Mutex<HeapPagingLinks>>,
    peak_allocated_bytes: PeakAllocatedBytes,
}

// `PartialPagableArc` allocates the heap beside a `OnceLock<DataKey>`, after
// `Arc`'s two counts; that allocation is one per sealed heap, so keep it in
// jemalloc's 192-byte class on 64-bit targets.
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(mem::align_of::<FrozenFrozenHeap>() == mem::align_of::<usize>());
    assert!(
        2 * mem::size_of::<usize>()
            + mem::size_of::<FrozenFrozenHeap>()
            + mem::size_of::<OnceLock<DataKey>>()
            <= 192
    );
};

/// A heap's dependencies, set once: at seal for a heap built here, when its
/// header is loaded for a heap restored from storage.
#[derive(Default)]
struct LazyHeapRefs(OnceLock<Box<[OwnedFrozen<()>]>>);

impl LazyHeapRefs {
    fn resident(refs: Box<[OwnedFrozen<()>]>) -> Self {
        Self(OnceLock::from(refs))
    }

    /// The dependencies, or none while the header is unloaded.
    fn get(&self) -> &[OwnedFrozen<()>] {
        self.0.get().map_or(&[], |refs| refs)
    }
}

impl Allocative for LazyHeapRefs {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        if let Some(refs) = self.0.get() {
            visitor.visit_field(allocative::Key::new("refs"), refs);
        }
        visitor.exit();
    }
}

/// Process-local identity of an exact `FrozenFrozenHeap` allocation.
#[derive(Debug, Clone, Copy, Dupe, PartialEq, Eq, Hash, Allocative)]
pub(crate) struct FrozenHeapPtr(usize);

impl FrozenHeapPtr {
    pub(crate) fn addr(self) -> usize {
        self.0
    }

    /// A synthetic pointer for tests that only need identity, never a
    /// dereference (e.g. wait-graph keys).
    #[cfg(test)]
    pub(crate) fn testing_new(addr: usize) -> FrozenHeapPtr {
        FrozenHeapPtr(addr)
    }
}

#[derive(Clone, Dupe, Allocative)]
pub(crate) struct WeakFrozenHeapRef(PartialPagableWeak<FrozenFrozenHeap>);

impl WeakFrozenHeapRef {
    pub(crate) fn is_expired(&self) -> bool {
        self.0.is_expired()
    }

    pub(crate) fn upgrade(&self) -> Option<FrozenHeapArc> {
        self.0.upgrade().map(|heap| FrozenHeapArc(Some(heap)))
    }

    pub(crate) fn heap_ptr(&self) -> FrozenHeapPtr {
        FrozenHeapPtr(self.0.as_ptr() as usize)
    }
}

// SAFETY: read-only access to already-allocated arena memory is safe across
// threads. Concurrent allocations during partial-deser are serialized by
// the claim mutex in `HeapDeserializationState`.
unsafe impl Sync for FrozenFrozenHeap {}
unsafe impl Send for FrozenFrozenHeap {}

impl FrozenFrozenHeap {
    /// See [`FrozenHeapArc::heap_ref_id`].
    fn heap_ref_id(&self) -> Option<HeapRefId> {
        Some(HeapRefId::new(
            self.name.as_ref()?,
            self.serialization_nonce,
        ))
    }

    /// A heap with a known identity and an empty arena, as restored from
    /// storage before its header is loaded. Its dependencies and values
    /// arrive with its data.
    fn new_from_identity(name: FrozenHeapName, nonce: HeapSerializationNonce) -> Self {
        FrozenFrozenHeap {
            serialization_nonce: nonce,
            arena: Arena::default(),
            refs: LazyHeapRefs::default(),
            name: Some(name),
            paging_links: OnceBox::new(),
            peak_allocated_bytes: PeakAllocatedBytes::new(None),
        }
    }

    /// The paging links, created on first use.
    fn paging_links(&self) -> MutexGuard<'_, HeapPagingLinks> {
        self.paging_links
            .get_or_init(Default::default)
            .lock()
            .expect("paging links lock poisoned")
    }

    /// The paging links, or `None` for a heap that never took part in paging.
    fn existing_paging_links(&self) -> Option<MutexGuard<'_, HeapPagingLinks>> {
        self.paging_links
            .get()
            .map(|links| links.lock().expect("paging links lock poisoned"))
    }

    /// Give a restored heap its deserialization state. `source` is `Some`
    /// for a skeleton whose data is still in storage.
    ///
    /// The arc is a stable allocation: `HeapDeserializationState` holds a raw
    /// pointer into `arena`, so the address must not move.
    fn install_deser_state(
        heap: &PartialPagableArc<Self>,
        scope: &Arc<StarlarkDeserScope>,
        heap_id: HeapRefId,
        source: Option<DataKey>,
    ) {
        let arena_ptr: *const Arena<ChunkAllocator> = &heap.arena;
        // SAFETY: `arena_ptr` points into `*heap`, whose arc keeps the state
        // and arena in one allocation lifetime; state lookup retains the heap
        // before borrowing this state.
        let state = Box::new(unsafe {
            HeapDeserializationState::new(scope.dupe(), heap_id, source, arena_ptr)
        });
        heap.arena.set_deserialization_state(state);
    }

    fn register_ser_state(&self, state: &Arc<StarlarkSerState>) -> pagable::Result<()> {
        let mut links = self.paging_links();
        let registered = &mut links.ser_states;
        registered.retain(|existing| existing.strong_count() != 0);
        let state = Arc::downgrade(state);
        if !registered
            .iter()
            .any(|existing| Weak::ptr_eq(existing, &state))
        {
            registered.push(state);
        }
        Ok(())
    }

    /// Serialization format:
    /// ```text
    /// [heap_name: FrozenHeapName]
    /// [serialization_nonce: HeapSerializationNonce]
    /// [refs_count: usize]
    /// for each ref:
    ///     [pagable serialized arc]
    /// [body_byte_length: u32 LE raw]     // bytes from end of header to end of value data
    /// [body_arc_count:   u32 LE raw]     // arcs serialized in the body region
    /// // ─── metadata_start captured here; everything below is parsed lazily ───
    /// [total_value_count: u32]   // drop_value_count + non_drop_value_count
    /// [total_count:      u32]            // number of values
    /// [drop_value_count: u32]            // value_index < drop_value_count ⇒ drop bump
    /// // Offset table — fixed-size, 8 raw LE bytes per entry.
    /// // (total_count + 1) entries: one per value + one sentinel end entry.
    /// // Written as placeholder, then patched via write_at after value data.
    /// // Entries are indexed by `value_index` (drop bump first, then non-drop).
    /// for i in 0..=total_count:
    ///   [stream_offset: u32 LE]          // relative to base_pos
    ///   [arc_offset:    u32 LE]          // relative to base arc_index
    /// // Metadata — postcard encoded, variable size.
    /// // Indexed by `value_index`. Bump kind for value_index `i` is
    /// // `Drop` if `i < drop_value_count`, otherwise `NonDrop`.
    /// for each value:
    ///   [deser_type_id: u32]    // sorted index into the vtable registry
    ///   [alloc_size: u32]
    /// // base_pos starts here — offsets are relative to this point.
    /// // Value data — postcard encoded, sequential.
    /// for each value:
    ///   [value_data...]
    /// ```
    fn serialize_inner(&self, serializer: &mut dyn PagableSerializer) -> crate::Result<()> {
        let heap_name = self
            .name
            .as_ref()
            .expect("The name of the FrozenFrozenHeap should exist in starlark pagable serialize");
        heap_name.pagable_serialize(serializer)?;
        self.serialization_nonce.pagable_serialize(serializer)?;

        // A restored heap reuses its data key, so only a heap built here -
        // whose refs were set at seal - reaches this point.
        let refs = self.refs.get();
        refs.len().pagable_serialize(serializer)?;
        for heap_ref in refs.iter() {
            heap_ref.pagable_serialize(serializer)?;
        }

        let drop_headers = self.arena.collect_drop_headers_ordered();
        let non_drop_headers = self.arena.collect_undrop_headers_ordered();
        let drop_value_count = drop_headers.len();
        let total_count = drop_value_count + non_drop_headers.len();

        // Write the body header placeholder: 8 raw LE bytes
        // (body_byte_length, body_arc_count). Patched after the rest of the
        // body is serialized.
        let body_header_pos = serializer.position();
        for _ in 0..8 {
            0u8.pagable_serialize(serializer)?;
        }

        // `metadata_start` is captured here so the lazy parser begins at here.
        let metadata_start = serializer.position();
        (total_count as u32).pagable_serialize(serializer)?;
        (drop_value_count as u32).pagable_serialize(serializer)?;

        // Write offset table placeholder: (value_count + 1) entries × 8 bytes each.
        // The extra entry is the end sentinel (total stream bytes + total arcs).
        // Entries are indexed by `value_index` (drop first, then non-drop).
        let table_pos = serializer.position();
        let table_entry_count = total_count + 1;
        let table_byte_size = table_entry_count * 8;
        for _ in 0..table_byte_size {
            0u8.pagable_serialize(serializer)?;
        }

        // Values in serialization order: drop bump first, then non-drop bump.
        // This ordering defines `value_index` consistently between ser and
        // deser. Bump kind for value_index `i` is implicit:
        // [0, drop_value_count) → Drop, [drop_value_count, total_count) → NonDrop.
        let all_headers_in_order: Vec<&&AValueHeader> =
            drop_headers.iter().chain(non_drop_headers.iter()).collect();

        // Write metadata for each value (postcard encoded).
        for header in &all_headers_in_order {
            let avalue = header.unpack();
            let alloc_size = avalue.memory_size().bytes();
            avalue
                .vtable()
                .deser_type_id
                .pagable_serialize(serializer)?;
            alloc_size.pagable_serialize(serializer)?;
        }

        // Record base_pos — all offsets are relative to here.
        let base_pos = serializer.position();
        // `OwnedFrozen<()>` registered this heap and its transitive dependencies
        // before deferring this Arc for serialization.
        let state = StarlarkSerializerImpl::get_or_create_state(serializer);
        let mut ctx = StarlarkSerializerImpl::new_with_root_ptr(
            serializer,
            state,
            FrozenHeapPtr(self as *const Self as usize),
        );

        // Serialize value data, recording start cursor per value.
        let mut entry_cursors: Vec<(u32, u32)> = Vec::with_capacity(table_entry_count);
        for (value_index, header) in all_headers_in_order.iter().enumerate() {
            let start = ctx.pagable().position();
            entry_cursors.push((
                (start.byte_pos - base_pos.byte_pos) as u32,
                (start.arc_index - base_pos.arc_index) as u32,
            ));
            let value = header.unpack();
            value.starlark_serialize(&mut ctx).map_err(|error| {
                self.enrich_value_serialization_error(
                    error,
                    heap_name,
                    value_index,
                    value.vtable().type_name,
                    &all_headers_in_order,
                )
            })?;
        }
        // End sentinel: position after all value data.
        let end = ctx.pagable().position();
        entry_cursors.push((
            (end.byte_pos - base_pos.byte_pos) as u32,
            (end.arc_index - base_pos.arc_index) as u32,
        ));

        // Patch the offset table with actual values.
        let mut table_bytes = vec![0u8; table_byte_size];
        for (i, (stream_offset, arc_offset)) in entry_cursors.iter().enumerate() {
            let off = i * 8;
            table_bytes[off..off + 4].copy_from_slice(&stream_offset.to_le_bytes());
            table_bytes[off + 4..off + 8].copy_from_slice(&arc_offset.to_le_bytes());
        }
        // SAFETY: table_pos.byte_pos points to the placeholder written earlier,
        // and table_bytes has the correct size.
        unsafe { ctx.pagable().write_at(table_pos.byte_pos, &table_bytes) };

        // Patch the body header: (body_byte_length, body_arc_count).
        let body_byte_length = (end.byte_pos - metadata_start.byte_pos) as u32;
        let body_arc_count = (end.arc_index - metadata_start.arc_index) as u32;
        let mut header_bytes = [0u8; 8];
        header_bytes[0..4].copy_from_slice(&body_byte_length.to_le_bytes());
        header_bytes[4..8].copy_from_slice(&body_arc_count.to_le_bytes());
        // SAFETY: body_header_pos points to the 8-byte placeholder.
        unsafe {
            ctx.pagable()
                .write_at(body_header_pos.byte_pos, &header_bytes)
        };

        Ok(())
    }

    /// Read the rest of a heap's header after its identity into `heap`:
    /// dependencies - bound into `scope` - and the body header, which the
    /// lazily parsed body is skipped by. Returns where that body starts.
    ///
    /// A dependency named by key becomes a skeleton: bound and retained from
    /// here on, read only if something resolves into it. Where the backend
    /// cannot name it by key it is read now, as it always was.
    fn read_refs_and_body_header<'de, D: PagableDeserializer<'de> + ?Sized>(
        heap: &PartialPagableArc<Self>,
        deserializer: &mut D,
        scope: &Arc<StarlarkDeserScope>,
    ) -> crate::Result<PagableCursor> {
        let refs_count = usize::pagable_deserialize(deserializer)?;
        let mut refs = Vec::with_capacity(refs_count);
        for _ in 0..refs_count {
            refs.push(FrozenHeapArc::deserialize_ref(
                deserializer,
                scope,
                HeaderLoad::Deferred,
            )?);
        }

        let mut header = [0u8; 8];
        for b in &mut header {
            *b = u8::pagable_deserialize(deserializer)?;
        }
        let body_byte_length =
            u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let body_arc_count =
            u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let metadata_start = deserializer.position();

        // SAFETY: the serialized body header records the exact cursor delta to
        // the end of this heap body.
        unsafe {
            deserializer.seek(PagableCursor {
                byte_pos: metadata_start.byte_pos + body_byte_length,
                arc_index: metadata_start.arc_index + body_arc_count,
            })
        };

        // Two threads can load one header; each produces the same skeletons
        // through the arc cache, so whichever set lands is right.
        let _ignored = heap.refs.0.set(refs.into_boxed_slice());
        Ok(metadata_start)
    }
}

pub(crate) fn cached_heap_deserialization_state_retained_bytes(
    storage: &PagableStorageHandle,
) -> usize {
    let heaps = storage.deserialized_arcs::<PartialPagableArc<FrozenFrozenHeap>>();
    // One builder must span every heap so its shared-allocation visited set prevents a scope or
    // recipe reachable from multiple states from being counted once per heap.
    let mut builder = FlameGraphBuilder::default();
    for heap in &heaps {
        if let Some(state) = heap.arena.deserialization_state() {
            builder.visit_root(state);
        }
    }
    builder.finish().flamegraph().total_size()
}

impl Drop for FrozenFrozenHeap {
    fn drop(&mut self) {
        let heap_ptr = FrozenHeapPtr(self as *const Self as usize);
        if let Some(state) = self.arena.deserialization_state() {
            state.unregister_heap(heap_ptr);
        }

        if let Some(links) = self.existing_paging_links() {
            let chunk_bases = self.arena.allocated_chunk_bases();
            for state in links.ser_states.iter().filter_map(Weak::upgrade) {
                state.unregister_heap(heap_ptr, chunk_bases.iter().copied());
            }
        }
    }
}

/// PagableSerialize for FrozenFrozenHeap — delegates to StarlarkSerialize
/// by creating a local StarlarkSerializerImpl from the pagable serializer.
impl PagableSerialize for FrozenFrozenHeap {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        self.serialize_inner(serializer)
            .map_err(|e| e.into_anyhow())
    }
}

/// Wire tags for a serialized heap handle.
const HEAP_REF_TAG_NONE: u8 = 0;
const HEAP_REF_TAG_ARC: u8 = 1;
const HEAP_REF_TAG_STATIC: u8 = 2;

/// The wire format of [`OwnedFrozen<()>`], which delegates here: the inner Arc via the pagable arc
/// mechanism.
///
/// Registered static heaps (`globals_static!` / `methods_static!`) are
/// referenced by `StaticHeapId` instead: their values serialize as
/// `StaticValueId`s, so the body is never needed on page-in, and paging the
/// process-wide arc would stamp a `DataKey` on shared state that other
/// serialization consumers cannot inline afterwards.
impl PagableSerialize for FrozenHeapArc {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        let Some(arc) = &self.0 else {
            return HEAP_REF_TAG_NONE.pagable_serialize(serializer);
        };
        if let Some(id) = get_static_heap_id(self) {
            HEAP_REF_TAG_STATIC.pagable_serialize(serializer)?;
            return id.pagable_serialize(serializer);
        }
        HEAP_REF_TAG_ARC.pagable_serialize(serializer)?;
        // The heap's identity - name and seal nonce - goes beside its slot,
        // outside its own data, so page-in can know which heap a slot names
        // without loading its header.
        arc.name
            .as_ref()
            .ok_or_else(|| pagable::Error::msg("a serialized frozen heap must have a name"))?
            .pagable_serialize(serializer)?;
        arc.serialization_nonce.pagable_serialize(serializer)?;
        let state = StarlarkSerializerImpl::get_or_create_state(serializer);
        state.ensure_chunk_index_registered(self)?;
        serializer.serialize_arc(arc)
    }
}

/// A heap handle outside a ref list - the owner ahead of an `OwnedFrozen`
/// value, say. Read in full: its values are about to be resolved.
impl<'de> PagableDeserialize<'de> for FrozenHeapArc {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let scope = StarlarkDeserializerImpl::get_or_create_scope(deserializer.as_dyn());
        Self::deserialize_ref(deserializer, &scope, HeaderLoad::Eager)
            .map(|owned| owned.heap_arc().dupe())
            .map_err(|e| e.into_anyhow())
    }
}

/// When a heap handle's header - its dependencies and where its body starts -
/// is loaded. Values are lazy either way.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HeaderLoad {
    /// Before the handle is returned.
    Eager,
    /// When something first resolves into the heap, if ever.
    Deferred,
}

impl FrozenHeapArc {
    /// Read one serialized heap handle: tag, name, arc slot.
    ///
    /// With an eager `header` the heap's header is loaded before returning;
    /// with a deferred one, a heap the backend can name by key is bound as a
    /// skeleton and loaded only when something resolves into it. Either way
    /// the heap and whatever of its graph is known are registered in `scope`.
    /// Values are lazy in both cases.
    fn deserialize_ref<'de, D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
        scope: &Arc<StarlarkDeserScope>,
        header: HeaderLoad,
    ) -> crate::Result<OwnedFrozen<()>> {
        let tag = u8::pagable_deserialize(deserializer)?;
        let heap = match tag {
            HEAP_REF_TAG_NONE => FrozenHeapArc::default(),
            HEAP_REF_TAG_ARC => {
                let name = FrozenHeapName::pagable_deserialize(deserializer)?;
                let nonce = HeapSerializationNonce::pagable_deserialize(deserializer)?;
                match take_arc_key(deserializer)? {
                    Some(arc_key) => {
                        let heap =
                            bind_skeleton(&deserializer.storage(), scope, name, nonce, arc_key)?;
                        if header == HeaderLoad::Eager {
                            heap.ensure_header_loaded(scope, &deserializer.storage())?;
                        }
                        heap
                    }
                    None => {
                        let arc_box = deserializer.deserialize_arc(
                            TypeId::of::<PartialPagableArc<FrozenFrozenHeap>>(),
                            deserialize_heap_arc_with_recipe,
                        )?;
                        let heap = downcast_heap_arc(&*arc_box)?;
                        let slot_id = HeapRefId::new(&name, nonce);
                        if heap.heap_ref_id() != Some(slot_id) {
                            return Err(pagable::Error::msg(format!(
                                "frozen heap: slot names {name} ({slot_id:?}) but its data holds {:?}",
                                heap.heap_ref_id()
                            ))
                            .into());
                        }
                        heap.register_heap_graph_in_deser_scope(scope)?;
                        heap
                    }
                }
            }
            HEAP_REF_TAG_STATIC => {
                let id = StaticHeapId::pagable_deserialize(deserializer)?;
                let heap = get_static_heap_by_id(id)
                    .ok_or_else(|| {
                        pagable::Error::msg(format!(
                            "frozen heap: static heap {id:?} is not registered in this process"
                        ))
                    })?
                    .dupe();
                // Publish the live heap graph so cross-heap value pointers into it (or its
                // dependencies) resolve.
                heap.register_heap_graph_in_deser_scope(scope)?;
                heap
            }
            _ => {
                return Err(
                    pagable::Error::msg(format!("frozen heap: invalid wire tag {tag}")).into(),
                );
            }
        };
        Ok(OwnedFrozen::for_heap(heap))
    }

    /// Load this heap's header if it is still in storage: its nonce, its
    /// dependencies - bound into `scope` as skeletons - and where its body
    /// starts. Slot metadata and values stay lazy, although the heap's whole
    /// serialized data is fetched to get at the header. A heap built here, or
    /// one whose header is loaded, returns at once.
    pub(crate) fn ensure_header_loaded(
        &self,
        scope: &Arc<StarlarkDeserScope>,
        storage: &PagableStorageHandle,
    ) -> crate::Result<()> {
        let Some(arc) = &self.0 else {
            return Ok(());
        };
        let Some(state) = arc.arena.deserialization_state() else {
            return Ok(());
        };
        if state.is_header_loaded() {
            return Ok(());
        }
        let Some(source) = state.source() else {
            return Ok(());
        };
        let recipe = fetch_data_recipe(storage, source)?;
        let metadata_start = {
            let mut de = recipe.open(storage);
            // The data repeats the identity the slot bound this skeleton under.
            let name = FrozenHeapName::pagable_deserialize(&mut *de)?;
            let nonce = HeapSerializationNonce::pagable_deserialize(&mut *de)?;
            let stored_id = HeapRefId::new(&name, nonce);
            if arc.heap_ref_id() != Some(stored_id) {
                return Err(pagable::Error::msg(format!(
                    "frozen heap: skeleton bound as {:?} but its data holds {name} ({stored_id:?})",
                    arc.heap_ref_id()
                ))
                .into());
            }
            FrozenFrozenHeap::read_refs_and_body_header(arc, &mut *de, scope)?
        };
        state.set_header(recipe, metadata_start);
        Ok(())
    }

    /// Whether this heap's dependencies are known: always for a heap built
    /// here, and for a restored one once its header is loaded.
    pub(crate) fn is_header_loaded(&self) -> bool {
        self.0.as_ref().is_none_or(|arc| {
            arc.arena
                .deserialization_state()
                .is_none_or(|state| state.is_header_loaded())
        })
    }

    /// Heaps held beyond the ref list, see [`Self::retain_dependency`].
    #[cfg(all(test, feature = "pagable", fbcode_build))]
    pub(crate) fn retained_beyond_refs(&self) -> Vec<FrozenHeapArc> {
        let links = self.0.as_ref().and_then(|arc| arc.existing_paging_links());
        links.map_or_else(Vec::new, |links| {
            links.retained.iter().map(|r| r.heap_arc().dupe()).collect()
        })
    }

    /// The heaps a value in this heap may point into: the ref list and the
    /// heaps retained beyond it, see [`Self::retain_dependency`].
    pub(crate) fn dependency_heaps(&self) -> Vec<FrozenHeapArc> {
        let Some(arc) = &self.0 else {
            return Vec::new();
        };
        let links = arc.existing_paging_links();
        let retained = links.as_ref().map_or(&[][..], |links| &links.retained);
        arc.refs
            .get()
            .iter()
            .chain(retained)
            .map(|r| r.heap_arc().dupe())
            .collect()
    }

    /// Hold `dep` alive for as long as this heap lives, for a pointer from a
    /// value here into `dep` that no ref list accounts for.
    ///
    /// A value may point into a transitive dependency, which its own heap's ref
    /// list does not name, and a lookup by identity may land in a heap outside
    /// the serialized dependency graph altogether. Normally the chain of ref
    /// lists keeps such a target alive, but lazy resolution can reach it before
    /// that chain is rebuilt. Retaining it here, on a cache hit as much as on a
    /// fetch, keeps its lifetime off the storage cache.
    pub(crate) fn retain_dependency(&self, dep: &FrozenHeapArc) {
        if self == dep {
            return;
        }
        let Some(arc) = &self.0 else {
            return;
        };
        if arc.refs.get().iter().any(|r| r.heap_arc() == dep) {
            return;
        }
        let mut links = arc.paging_links();
        if !links.retained.iter().any(|r| r.heap_arc() == dep) {
            links.retained.push(OwnedFrozen::for_heap(dep.dupe()));
        }
    }
}

/// Bind the heap stored under `key` into `scope` without loading its header.
///
/// The arc cache decides whether that is a new skeleton or a heap some earlier
/// page-in already holds - possibly one that never left memory - so every
/// referrer of the key shares one allocation. Whatever of the heap's graph is
/// known is registered along with it.
///
/// Unreachable where the OSS build's published `pagable` predates the lazy
/// binding API: without `take_arc_key` no key ever reaches here.
#[cfg_attr(
    not(fbcode_build),
    expect(unused_variables, reason = "the fbcode-only body is the only reader")
)]
fn bind_skeleton(
    storage: &PagableStorageHandle,
    scope: &Arc<StarlarkDeserScope>,
    name: FrozenHeapName,
    nonce: HeapSerializationNonce,
    key: DataKey,
) -> crate::Result<FrozenHeapArc> {
    #[cfg(fbcode_build)]
    {
        let arc_box = storage.bind_arc_by_key_lazily(
            key,
            TypeId::of::<PartialPagableArc<FrozenFrozenHeap>>(),
            || {
                let heap_id = HeapRefId::new(&name, nonce);
                let heap = PartialPagableArc::new(FrozenFrozenHeap::new_from_identity(name, nonce));
                FrozenFrozenHeap::install_deser_state(&heap, scope, heap_id, Some(key));
                Box::new(heap)
            },
        );
        let heap = downcast_heap_arc(&*arc_box)?;
        heap.register_heap_graph_in_deser_scope(scope)?;
        Ok(heap)
    }
    #[cfg(not(fbcode_build))]
    unreachable!("no heap is bound by key without `take_arc_key`")
}

/// See [`bind_skeleton`] for why this is fbcode-only.
#[cfg_attr(
    not(fbcode_build),
    expect(unused_variables, reason = "the fbcode-only body is the only reader")
)]
fn fetch_data_recipe(
    storage: &PagableStorageHandle,
    key: DataKey,
) -> crate::Result<Arc<dyn pagable::PagableDeserializerRecipe>> {
    #[cfg(fbcode_build)]
    return storage
        .fetch_recipe_by_key(key)
        .map_err(crate::Error::new_other);
    #[cfg(not(fbcode_build))]
    unreachable!("no header is left unloaded without `take_arc_key`")
}

/// `deserializer.take_arc_key()`, compiled out where the OSS build's published
/// `pagable` predates the method. `None` there reads every heap eagerly, the
/// behavior before skeletons.
fn take_arc_key<'de, D: PagableDeserializer<'de> + ?Sized>(
    deserializer: &mut D,
) -> pagable::Result<Option<DataKey>> {
    #[cfg(fbcode_build)]
    return deserializer.take_arc_key();
    #[cfg(not(fbcode_build))]
    {
        let _ = deserializer;
        Ok(None)
    }
}

/// `deserialize_arc` callback for a heap read in place: builds the heap and
/// loads its header before returning it. The recipe reopens the data for the
/// lazily parsed body.
pub(crate) fn deserialize_heap_arc_with_recipe(
    de: &mut dyn PagableDeserializer<'_>,
    recipe: Arc<dyn pagable::PagableDeserializerRecipe>,
) -> pagable::Result<Box<dyn pagable::arc_erase::ArcEraseDyn>> {
    let scope = StarlarkDeserializerImpl::get_or_create_scope(de);
    let name = FrozenHeapName::pagable_deserialize(de)?;
    let nonce = HeapSerializationNonce::pagable_deserialize(de)?;
    let heap_id = HeapRefId::new(&name, nonce);
    let heap = PartialPagableArc::new(FrozenFrozenHeap::new_from_identity(name, nonce));
    FrozenFrozenHeap::install_deser_state(&heap, &scope, heap_id, None);
    let metadata_start = FrozenFrozenHeap::read_refs_and_body_header(&heap, de, &scope)
        .map_err(|e| e.into_anyhow())?;
    heap.arena
        .deserialization_state()
        .expect("installed above")
        .set_header(recipe, metadata_start);
    scope.register_heap(
        heap_id,
        WeakFrozenHeapRef(PartialPagableArc::downgrade(&heap)),
    )?;
    Ok(Box::new(heap))
}

/// Recover a `FrozenHeapArc` from the type-erased arc the arc cache hands out.
fn downcast_heap_arc(arc: &dyn ArcEraseDyn) -> crate::Result<FrozenHeapArc> {
    let arc = arc
        .as_arc_any()
        .downcast_ref::<PartialPagableArc<FrozenFrozenHeap>>()
        .ok_or_else(|| {
            pagable::Error::msg(
                "frozen heap: type mismatch downcasting PartialPagableArc<FrozenFrozenHeap>",
            )
        })?
        .clone();
    Ok(FrozenHeapArc(Some(arc)))
}

impl Debug for FrozenFrozenHeap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut x = f.debug_struct("FrozenHeap");
        x.field("serialization_nonce", &self.serialization_nonce);
        x.field("bytes", &self.arena.allocated_bytes());
        x.field("refs", &self.refs.get().len());
        x.field("header_loaded", &self.refs.0.get().is_some());
        x.finish()
    }
}

/// The shared allocation behind a sealed [`FrozenHeap`], keeping alive all values on it; `None` is
/// the empty heap.
///
/// This is the owner inside every [`OwnedFrozen`]. The public handle to it is [`OwnedFrozen<()>`],
/// whose identity semantics are these: [`Hash`]/[`Eq`] by allocation, consistent within a process
/// but non-deterministic across executions and between distinct but observably identical heaps.
#[derive(Default, Clone, Dupe, Debug, Allocative)]
pub(crate) struct FrozenHeapArc(Option<PartialPagableArc<FrozenFrozenHeap>>);

fn _test_frozen_heap_arc_send_sync()
where
    FrozenHeapArc: Send + Sync,
{
}

impl Hash for FrozenHeapArc {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if let Some(arc) = &self.0 {
            let x: &FrozenFrozenHeap = Deref::deref(&arc);
            ptr::hash(x, state);
        }
    }
}

impl PartialEq for FrozenHeapArc {
    fn eq(&self, other: &FrozenHeapArc) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => PartialPagableArc::ptr_eq(a, b),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    }
}

impl Eq for FrozenHeapArc {}

/// How page-in obtained a heap allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeapAllocationOrigin {
    /// Reconstructed from stored bytes.
    Deserialized,
    /// A live allocation page-in reused instead of reconstructing.
    Native,
}

impl HeapAllocationOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Deserialized => "deserialized",
            Self::Native => "native",
        }
    }
}

impl FrozenHeapArc {
    /// Seal `arena` with the heaps it depends on.
    pub(in crate::values::layout::heap) fn new_sealed(
        arena: Arena<ChunkAllocator>,
        refs: Box<[OwnedFrozen<()>]>,
        name: Option<FrozenHeapName>,
        peak_allocated_bytes: Option<usize>,
    ) -> Self {
        Self(Some(PartialPagableArc::new(FrozenFrozenHeap {
            serialization_nonce: HeapSerializationNonce::random(),
            arena,
            refs: LazyHeapRefs::resident(refs),
            name,
            paging_links: OnceBox::new(),
            peak_allocated_bytes: PeakAllocatedBytes::new(peak_allocated_bytes),
        })))
    }

    /// Whether this is the empty heap, which keeps nothing alive.
    pub(in crate::values::layout::heap) fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    /// Only page-in installs a `HeapDeserializationState`, so its presence
    /// separates a reconstructed allocation from a live one.
    pub(crate) fn allocation_origin(&self) -> HeapAllocationOrigin {
        match self.deser_state() {
            Some(_) => HeapAllocationOrigin::Deserialized,
            None => HeapAllocationOrigin::Native,
        }
    }

    pub(crate) fn deser_state(&self) -> Option<&HeapDeserializationState> {
        self.0
            .as_ref()
            .and_then(|heap| heap.arena.deserialization_state())
    }

    pub(crate) fn register_heap_graph_in_deser_scope(
        &self,
        scope: &StarlarkDeserScope,
    ) -> pagable::Result<()> {
        let heap_id = self
            .heap_ref_id()
            .ok_or_else(|| pagable::Error::msg("deserialized frozen heap must have a name"))?;
        let heap = self
            .downgrade()
            .expect("a deserialized heap must have an inner allocation");
        if scope
            .is_heap_bound(heap_id, self)
            .map_err(pagable::Error::new)?
        {
            return Ok(());
        }

        // A cached owner can contain value pointers into any transitive
        // dependency, so publish the complete graph before the owner binding.
        for dep in self.dependency_heaps() {
            dep.register_heap_graph_in_deser_scope(scope)?;
        }

        scope
            .register_heap(heap_id, heap)
            .map_err(pagable::Error::new)
    }

    // The public identity API on `OwnedFrozen`/`OwnedFrozenRef` forwards to the following.

    pub(crate) fn allocated_bytes(&self) -> usize {
        self.0.as_ref().map_or(0, |a| a.arena.allocated_bytes())
    }

    pub(crate) fn peak_allocated_bytes(&self) -> Option<usize> {
        self.0.as_ref().and_then(|a| a.peak_allocated_bytes.get())
    }

    pub(crate) fn available_bytes(&self) -> usize {
        self.0.as_ref().map_or(0, |a| a.arena.available_bytes())
    }

    pub(crate) fn allocated_summary(&self) -> HeapSummary {
        self.0
            .as_ref()
            .map_or_else(HeapSummary::default, |a| a.arena.allocated_summary())
    }

    pub(crate) fn name(&self) -> Option<&FrozenHeapName> {
        self.0.as_ref().and_then(|a| a.name.as_ref())
    }

    /// The identity pointers into this heap are written against: its name and
    /// the nonce drawn when it was sealed. `None` for an unnamed heap, which
    /// cannot be serialized.
    pub(crate) fn heap_ref_id(&self) -> Option<HeapRefId> {
        self.0.as_ref()?.heap_ref_id()
    }

    #[cfg(test)]
    pub(crate) fn serialization_nonce(&self) -> Option<HeapSerializationNonce> {
        Some(self.0.as_ref()?.serialization_nonce)
    }

    /// The frozen heaps that this frozen heap depends on. Empty for a heap
    /// restored from storage whose header is not loaded, see
    /// [`Self::is_header_loaded`].
    pub(crate) fn refs_slice(&self) -> &[OwnedFrozen<()>] {
        match &self.0 {
            Some(inner) => inner.refs.get(),
            None => &[],
        }
    }

    pub(crate) fn register_ser_state(&self, state: &Arc<StarlarkSerState>) -> pagable::Result<()> {
        if let Some(inner) = &self.0 {
            inner.register_ser_state(state)?;
        }
        Ok(())
    }

    pub(crate) fn downgrade(&self) -> Option<WeakFrozenHeapRef> {
        self.0
            .as_ref()
            .map(|heap| WeakFrozenHeapRef(PartialPagableArc::downgrade(heap)))
    }

    /// The initialized values in this heap, borrowed for the lifetime of the heap reference.
    pub(crate) fn iter_values(&self) -> impl Iterator<Item = Value<'_>> {
        struct ValueCollector<'v>(Vec<Value<'v>>);
        let mut items = ValueCollector(Vec::new());
        if let Some(heap) = &self.0 {
            impl<'v> ArenaVisitor<'v> for ValueCollector<'v> {
                fn enter_bump(&mut self) {}

                fn regular_entry(&mut self, entry: ArenaEntry<'v>) {
                    if let ArenaEntry::Value(header) = entry {
                        // SAFETY: the decoded entry is initialized and its frozen heap
                        // remains borrowed for the lifetime of the returned value.
                        self.0
                            .push(unsafe { header.unpack_value(HeapKind::Frozen) });
                    }
                }

                fn call_enter(&mut self, _function: Value<'v>, _time: ProfilerInstant) {}

                fn call_exit(&mut self, _time: ProfilerInstant) {}
            }
            unsafe {
                heap.arena
                    .visit_arena(HeapKind::Frozen, HeapKind::Frozen, &mut items)
            };
        }
        items.0.into_iter()
    }

    /// See [`Arena::build_chunk_index`].
    pub(crate) fn build_chunk_index(&self) -> Vec<ChunkInfo> {
        match &self.0 {
            Some(inner) => inner.arena.build_chunk_index(),
            None => Vec::new(),
        }
    }

    /// Collect live value headers from the non-drop bump in allocation order.
    #[cfg(all(test, feature = "pagable"))]
    pub(crate) fn collect_undrop_headers_ordered(&self) -> Vec<&AValueHeader> {
        match &self.0 {
            Some(inner) => inner.arena.collect_undrop_headers_ordered(),
            None => Vec::new(),
        }
    }

    /// Collect live value headers from the drop bump in allocation order.
    #[cfg(all(test, feature = "pagable"))]
    pub(crate) fn collect_drop_headers_ordered(&self) -> Vec<&AValueHeader> {
        match &self.0 {
            Some(inner) => inner.arena.collect_drop_headers_ordered(),
            None => Vec::new(),
        }
    }
}

// Error-path diagnostics for unresolved frozen pointers. These types and
// searches are not used by successful serialization.
#[derive(Debug, derive_more::Display)]
enum FrozenValueIndexDiagnostic {
    #[display(
        "target_physical_index={physical_index}, target_original_recipe_index=<native heap>, target_type={value_type}, target_owner_is_restored=false"
    )]
    Native {
        physical_index: usize,
        value_type: &'static str,
    },
    #[display(
        "target_physical_index=<restored allocation order is not wire order>, target_original_recipe_index={original_recipe_index}, target_owner_is_restored=true"
    )]
    Restored { original_recipe_index: u32 },
}

#[derive(Debug, derive_more::Display)]
#[display("target_owner_heap={heap_name}, target_owner_ptr={heap_ptr:#x}, {index}")]
pub(crate) struct FrozenValueLocationDiagnostic {
    heap_name: String,
    heap_ptr: usize,
    index: FrozenValueIndexDiagnostic,
}

pub(crate) enum FrozenValueOwnerSearchResult {
    Found {
        location: FrozenValueLocationDiagnostic,
        heaps_scanned: usize,
    },
    NotFound {
        heaps_scanned: usize,
    },
}

impl FrozenFrozenHeap {
    #[cold]
    fn locate_value_for_diagnostic(&self, raw_ptr: usize) -> Option<FrozenValueLocationDiagnostic> {
        let heap_ptr = self as *const Self as usize;
        let heap_name = self
            .name
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "<unnamed>".to_owned());

        if let Some(state) = self.arena.deserialization_state() {
            let original_recipe_index = state.original_value_index(raw_ptr)?;
            return Some(FrozenValueLocationDiagnostic {
                heap_name,
                heap_ptr,
                index: FrozenValueIndexDiagnostic::Restored {
                    original_recipe_index,
                },
            });
        }

        let drop_headers = self.arena.collect_drop_headers_ordered();
        let non_drop_headers = self.arena.collect_undrop_headers_ordered();
        let (physical_index, header) = drop_headers
            .iter()
            .chain(non_drop_headers.iter())
            .enumerate()
            .find(|(_, header)| header.payload_ptr().ptr as usize == raw_ptr)?;
        Some(FrozenValueLocationDiagnostic {
            heap_name,
            heap_ptr,
            index: FrozenValueIndexDiagnostic::Native {
                physical_index,
                value_type: header.unpack().vtable().type_name,
            },
        })
    }

    #[cold]
    fn find_reachable_value_for_diagnostic(&self, raw_ptr: usize) -> FrozenValueOwnerSearchResult {
        let mut pending = vec![self];
        let mut visited = HashSet::new();

        while let Some(heap) = pending.pop() {
            let heap_ptr = heap as *const Self as usize;
            if !visited.insert(heap_ptr) {
                continue;
            }

            if let Some(location) = heap.locate_value_for_diagnostic(raw_ptr) {
                return FrozenValueOwnerSearchResult::Found {
                    location,
                    heaps_scanned: visited.len(),
                };
            }

            for heap_ref in heap.refs.get().iter() {
                if let Some(referenced_heap) = heap_ref.heap_arc().0.as_ref() {
                    pending.push(Deref::deref(referenced_heap));
                }
            }
        }

        FrozenValueOwnerSearchResult::NotFound {
            heaps_scanned: visited.len(),
        }
    }

    #[cold]
    fn enrich_value_serialization_error(
        &self,
        error: crate::Error,
        heap_name: &FrozenHeapName,
        value_index: usize,
        value_type: &'static str,
        all_headers_in_order: &[&&AValueHeader],
    ) -> crate::Error {
        let heap_ptr = self as *const Self as usize;
        let missing_target = match error.kind() {
            crate::ErrorKind::Other(error) => match error.downcast_ref::<PagableError>() {
                Some(PagableError::FrozenValueNotRegistered { raw_ptr, .. }) => Some(*raw_ptr),
                _ => None,
            },
            _ => None,
        };
        let target_diagnostic = missing_target.map_or_else(
            || "target location is unavailable for this error".to_owned(),
            |raw_ptr| {
                let source_snapshot_index = all_headers_in_order
                    .iter()
                    .position(|header| header.payload_ptr().ptr as usize == raw_ptr);
                let owner_diagnostic = match self.find_reachable_value_for_diagnostic(raw_ptr) {
                    FrozenValueOwnerSearchResult::Found { location, .. } => location.to_string(),
                    FrozenValueOwnerSearchResult::NotFound { heaps_scanned } => format!(
                        "target owner was not found in the source heap or its {heaps_scanned} reachable heap allocations",
                    ),
                };
                format!(
                    "{owner_diagnostic}; target_source_snapshot_index={source_snapshot_index:?}",
                )
            },
        );

        error.with_context(format!(
            "serializing heap `{heap_name}` allocation {heap_ptr:#x} value_index {value_index} type `{value_type}`; source_heap_is_restored={}, direct_ref_count={}; {target_diagnostic}",
            self.arena.deserialization_state().is_some(),
            self.refs.get().len(),
        ))
    }
}

impl FrozenHeapArc {
    #[cold]
    pub(crate) fn locate_value_for_diagnostic(
        &self,
        raw_ptr: usize,
    ) -> Option<FrozenValueLocationDiagnostic> {
        self.0
            .as_ref()
            .and_then(|heap| heap.locate_value_for_diagnostic(raw_ptr))
    }
}
