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

//! Implementation of StarlarkDeserializeContext.

use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::num::NonZeroU32;
use std::ptr;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
#[cfg(all(test, feature = "pagable"))]
use std::sync::mpsc::Sender;
use std::thread::ThreadId;

use allocative::Allocative;
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use derive_more::From;
use dupe::Dupe;
use pagable::DataKey;
#[cfg(fbcode_build)]
use pagable::DeferredReadFailures;
use pagable::PagableCursor;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableDeserializerRecipe;
use pagable::StorageContext;
use pagable::StorageState;
use pagable::storage::handle::PagableStorageHandle;

use crate::pagable::DeserTypeId;
use crate::pagable::error::PagableError;
use crate::pagable::heap_ref_id::HeapRefId;
use crate::pagable::lookup_vtable;
use crate::pagable::serialized_frozen_value::SerializedFrozenValue;
use crate::pagable::starlark_deserialize::StarlarkDeserialize;
use crate::pagable::starlark_deserialize::StarlarkDeserializeContext;
use crate::pagable::starlark_serialize_context::StarlarkSerState;
use crate::pagable::static_value::get_static_value_by_id;
use crate::values::Value;
#[cfg(fbcode_build)]
use crate::values::deferred::DeferredReadContext;
use crate::values::layout::aligned_size::AlignedSize;
use crate::values::layout::heap::allocator::alloc::allocator::ChunkAllocator;
use crate::values::layout::heap::arena::Arena;
use crate::values::layout::heap::arena::BumpKind;
use crate::values::layout::heap::arena::ChunkInfo;
use crate::values::layout::heap::edge::HeapEdge;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::sealed::FrozenHeapArc;
use crate::values::layout::heap::sealed::FrozenHeapPtr;
use crate::values::layout::heap::sealed::HeapAllocationOrigin;
use crate::values::layout::heap::sealed::WeakFrozenHeapRef;
use crate::values::layout::heap::sealed::cached_heap_deserialization_state_retained_bytes;
use crate::values::layout::heap::sealed::load_and_bind_heap_by_id;
use crate::values::layout::heap::sealed::register_heap_key_index;
use crate::values::layout::pointer::PointerTags;
use crate::values::layout::value_alloc_size::ValueAllocSize;
use crate::values::layout::vtable::AValueVTable;
use crate::values::layout::vtable::StarlarkValueRawPtr;
use crate::values::types::int::inline_int::InlineInt;

mod readiness;

use readiness::ActiveClaim;
use readiness::ReadinessGraph;

/// Deserialize a field whose payload will be inspected during construction.
/// Unlike an opaque graph edge, this requires transitive readiness.
pub(crate) fn deserialize_for_inspection<'fv, T: StarlarkDeserialize<'fv>>(
    ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
) -> crate::Result<T> {
    let _root = ActiveClaim::root();
    T::starlark_deserialize(ctx)
}

/// Per-slot metadata for partial-deser. Immutable after `deserialize_metadata`.
#[derive(Allocative)]
pub(crate) struct ValueDeserSlot {
    /// Byte offset of this value's data relative to base_pos.
    stream_offset: u32,
    /// Arc index offset relative to base_pos.arc_index.
    arc_offset: u32,
    /// This value's vtable, used for deserialization dispatch.
    #[allocative(skip)]
    vtable: &'static AValueVTable,
    /// Which bump (drop or non-drop) this value lives in.
    bump_kind: BumpKind,
    /// Size in bytes to allocate for this value's header + payload.
    alloc_size: NonZeroU32,
}

impl ValueDeserSlot {
    pub(crate) fn new(
        stream_offset: u32,
        arc_offset: u32,
        vtable: &'static AValueVTable,
        bump_kind: BumpKind,
        alloc_size: NonZeroU32,
    ) -> Self {
        Self {
            stream_offset,
            arc_offset,
            vtable,
            bump_kind,
            alloc_size,
        }
    }
}

/// The recipe and allocation owned by a [`SlotClaimGuard`].
struct DeserializeRecipe {
    /// The slot's index in the heap's metadata.
    index: usize,
    /// Absolute cursor position of this value's data.
    abs_pos: PagableCursor,
    /// Vtable for deserialization dispatch.
    vtable: &'static AValueVTable,
    /// Raw pointer to the pre-allocated payload in the arena.
    raw_ptr: StarlarkValueRawPtr,
    /// Pointer to the AValueHeader in the arena (for vtable patching after deserialization).
    header_ptr: NonNull<AValueHeader>,
}

impl DeserializeRecipe {
    /// Write the real vtable to the header, replacing the sentinel.
    ///
    /// # Safety
    /// `starlark_deserialize` must have successfully initialized the payload.
    unsafe fn write_vtable_to_header(&self) {
        // Release store, paired with the acquire load an arena walk decodes
        // headers with, so a walk sees the payload before the vtable.
        // SAFETY: `header_ptr` is the claim's aligned header word.
        unsafe {
            (*(self.header_ptr.as_ptr() as *const AtomicPtr<AValueVTable>)).store(
                self.vtable as *const AValueVTable as *mut AValueVTable,
                Ordering::Release,
            );
        }
    }
}

/// A won claim bound to its heap state. Dropping an unfinished claim publishes
/// failure so waiters are not stranded by an early return or unwinding panic.
struct SlotClaimGuard<'a> {
    state: &'a HeapDeserializationState,
    value: HeapValueId,
    claim: Option<DeserializeRecipe>,
}

impl SlotClaimGuard<'_> {
    fn recipe(&self) -> &DeserializeRecipe {
        self.claim.as_ref().expect("claim guard is still armed")
    }

    /// # Safety
    /// The recipe's deserializer must have successfully initialized the payload.
    unsafe fn publish(mut self) -> crate::Result<SlotReadiness> {
        let claim = self.claim.take().expect("claim guard is consumed once");
        // SAFETY: `publish` requires this recipe's payload to be successfully
        // initialized, satisfying `write_vtable_to_header`'s precondition.
        unsafe { claim.write_vtable_to_header() };
        self.state.finalize_claim(self.value, claim)
    }

    fn abort(mut self, error: &crate::Error) {
        self.state.abort_claim(
            self.value,
            self.claim.take().expect("claim guard is consumed once"),
            error,
        );
    }
}

impl Drop for SlotClaimGuard<'_> {
    fn drop(&mut self) {
        if let Some(claim) = self.claim.take() {
            self.state.abort_claim(
                self.value,
                claim,
                &crate::Error::new_other(anyhow::anyhow!("value deserialization did not complete")),
            );
        }
    }
}

/// Decoded form of an [`AtomicSlotState`].
enum SlotState {
    NotStarted,
    /// Claimed and mid-deserialization; carries the pre-allocated header (its
    /// vtable may still be the sentinel).
    InProgress(*mut AValueHeader),
    /// The payload is initialized, but a construction dependency is not ready.
    Constructed(*mut AValueHeader),
    Failed,
    Ready(*mut AValueHeader),
}

impl SlotState {
    /// Header pointer if the slot and its construction dependencies are ready.
    fn ready_ptr(self) -> Option<*mut AValueHeader> {
        match self {
            SlotState::Ready(ptr) => Some(ptr),
            _ => None,
        }
    }

    /// [`ClaimResult`] for a slot observed by a non-winning caller, or `None` if
    /// it is not started yet (the caller should attempt to claim it).
    fn observed_claim_result<'a>(self) -> Option<ClaimResult<'a>> {
        match self {
            SlotState::NotStarted => None,
            SlotState::InProgress(ptr) => Some(ClaimResult::InProgress(ptr)),
            SlotState::Constructed(ptr) => Some(ClaimResult::Resolved(SlotReadiness::Pending(ptr))),
            SlotState::Failed => Some(ClaimResult::Failed),
            SlotState::Ready(ptr) => Some(ClaimResult::Resolved(SlotReadiness::Ready(ptr))),
        }
    }
}

/// Per-slot init state and the single owner of the [`SlotState`] encoding, which
/// packs the initialization states into one pointer-sized word, the header
/// pointer with flags in its low bits:
/// 1. null (`INIT_NOT_STARTED`) — not started.
/// 2. in progress — bit 0 (`IN_PROGRESS_FLAG`) set; the header pointer is in the
///    remaining bits.
/// 3. ready — any other non-null value (all low bits clear); the value *is* the
///    header pointer.
/// 4. `0b10` (`INIT_FAILED_FLAG`) — failed.
/// 5. bit 2 (`READINESS_PENDING`) — enrolled in cycle readiness tracking. With
///    bit 0 clear, the payload is constructed but not yet safe to publish.
///
/// The low bits are free for the flags because `AValueHeader` is ≥ 8-byte
/// aligned (checked below).
/// After the initial claim, mutations require a [`HeapPublication`] guard.
/// Its mutex serializes enrollment with completion; readers outside that
/// mutex use acquire loads to observe payload writes published by release stores.
#[repr(transparent)]
struct AtomicSlotState(AtomicPtr<AValueHeader>);

impl Allocative for AtomicSlotState {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        visitor.visit_simple_sized::<Self>();
    }
}

const _: () = {
    assert!(AValueHeader::ALIGN > AtomicSlotState::INIT_STATE_MASK);
};

impl AtomicSlotState {
    const INIT_NOT_STARTED: usize = 0;
    const IN_PROGRESS_FLAG: usize = 0b1;
    const INIT_FAILED_FLAG: usize = 0b10;
    const READINESS_PENDING: usize = 0b100;
    const INIT_STATE_MASK: usize = 0b111;

    fn not_started() -> Self {
        AtomicSlotState(AtomicPtr::new(ptr::null_mut()))
    }

    fn load(&self, order: Ordering) -> SlotState {
        let v = self.0.load(order);
        let bits = v.addr();
        if bits == Self::INIT_NOT_STARTED {
            SlotState::NotStarted
        } else if bits == Self::INIT_FAILED_FLAG {
            SlotState::Failed
        } else if bits & Self::IN_PROGRESS_FLAG != 0 {
            SlotState::InProgress(v.map_addr(|addr| addr & !Self::INIT_STATE_MASK))
        } else if bits & Self::READINESS_PENDING != 0 {
            SlotState::Constructed(v.map_addr(|addr| addr & !Self::INIT_STATE_MASK))
        } else {
            SlotState::Ready(v)
        }
    }

    /// Publish the claim: store the pre-allocated `header` with the in-progress
    /// flag set. The caller holds the claim lock, so this is the not-started ->
    /// in-progress transition.
    fn publish_in_progress(&self, header: *mut AValueHeader) {
        self.0.store(
            header.map_addr(|addr| addr | Self::IN_PROGRESS_FLAG),
            Ordering::Release,
        );
    }

    /// The publication mutex excludes concurrent readiness enrollment.
    fn try_finalize(&self, header: *mut AValueHeader) -> bool {
        if self.0.load(Ordering::Relaxed) != header.map_addr(|addr| addr | Self::IN_PROGRESS_FLAG) {
            return false;
        }
        self.0.store(header, Ordering::Release);
        true
    }

    /// Requires both the readiness and publication locks, in that order.
    fn track_readiness(&self) -> SlotState {
        match self.load(Ordering::Relaxed) {
            SlotState::InProgress(ptr) => {
                self.0.store(
                    ptr.map_addr(|addr| addr | Self::IN_PROGRESS_FLAG | Self::READINESS_PENDING),
                    Ordering::Release,
                );
                SlotState::InProgress(ptr)
            }
            state => state,
        }
    }

    fn constructed(&self, header: *mut AValueHeader) {
        self.0.store(
            header.map_addr(|addr| addr | Self::READINESS_PENDING),
            Ordering::Release,
        );
    }

    fn ready(&self) {
        let previous = self.0.load(Ordering::Relaxed);
        debug_assert_ne!(previous.addr() & Self::READINESS_PENDING, 0);
        // The group's last writer can go straight from in-progress to ready.
        self.0.store(
            previous.map_addr(|addr| addr & !Self::INIT_STATE_MASK),
            Ordering::Release,
        );
    }

    /// Publish the claim as failed.
    fn fail(&self) {
        self.0.store(
            ptr::without_provenance_mut(Self::INIT_FAILED_FLAG),
            Ordering::Release,
        );
    }
}

#[derive(Allocative)]
struct InitWaiters {
    state: Mutex<InitWaiterState>,
    #[allocative(skip)]
    cv: OnceLock<Condvar>,
}

#[derive(Allocative, Default)]
struct InitWaiterState {
    /// Failure records are sparse because successful slots never need this
    /// diagnostic state.
    failures: Vec<SlotFailure>,
    // Used by `test_panicking_deserializer_fails_claim_and_wakes_waiter` and
    // `cycle_readiness::cross_thread_cycle` to park a waiter before publication.
    #[cfg(all(test, feature = "pagable"))]
    #[allocative(skip)]
    wait_started: Option<(Option<ThreadId>, Sender<()>)>,
}

#[derive(Allocative)]
struct SlotFailure {
    index: usize,
    cause: Arc<str>,
}

impl InitWaiters {
    fn new() -> Self {
        Self {
            state: Mutex::new(InitWaiterState::default()),
            cv: OnceLock::new(),
        }
    }
}

enum ClaimResult<'a> {
    Claimed(SlotClaimGuard<'a>),
    Resolved(SlotReadiness),
    /// Slot is mid-deserialization. Carries its pre-allocated header, whose
    /// vtable may still be the sentinel.
    InProgress(*mut AValueHeader),
    Failed,
}

enum SlotReadiness {
    Ready(*mut AValueHeader),
    /// Locally constructed, but a construction dependency is not ready yet.
    Pending(*mut AValueHeader),
}

enum WaitFor {
    Constructed,
    Ready,
}

/// Metadata + init state — lazily parsed from the recipe on first
/// `try_claim`.
#[derive(Allocative)]
pub(crate) struct HeapMetadata {
    /// All values in this heap.
    slots: Vec<ValueDeserSlot>,
    /// Absolute cursor position of value data start (base for relative offsets).
    base_pos: PagableCursor,
    /// Per-slot init state. See [`AtomicSlotState`].
    init_states: Vec<AtomicSlotState>,
    /// Reverse map from each claimed payload address to its original recipe
    /// index. Lazy allocation order is not necessarily recipe order.
    ///
    /// Populated when a slot is claimed: the mapping is a
    /// property of the arena allocation, and recording it up front means no
    /// reader can observe a payload address without also finding its index. An
    /// aborted claim therefore leaves a correct-but-unreferenced entry, which is
    /// preferable to the address resolving to nothing.
    original_indices_by_payload: RwLock<HashMap<usize, u32>>,
    /// Coordinates waiters that lost a per-slot initialization race.
    init_waiters: InitWaiters,
}

/// Serializes slot transitions and wakes waiters once for the whole batch.
/// When both locks are needed, acquire readiness before this heap's waiter lock.
struct HeapPublication<'a> {
    metadata: &'a HeapMetadata,
    waiters: MutexGuard<'a, InitWaiterState>,
    changed: bool,
}

impl HeapPublication<'_> {
    fn try_finalize(&mut self, index: usize, header: *mut AValueHeader) -> bool {
        let changed = self.metadata.init_states[index].try_finalize(header);
        self.changed |= changed;
        changed
    }

    fn track_readiness(&mut self, index: usize) -> SlotState {
        // Enrollment alone does not satisfy either kind of waiter.
        self.metadata.init_states[index].track_readiness()
    }

    fn constructed(&mut self, index: usize, header: *mut AValueHeader) {
        self.metadata.init_states[index].constructed(header);
        self.changed = true;
    }

    fn ready(&mut self, index: usize) {
        self.metadata.init_states[index].ready();
        self.changed = true;
    }

    fn fail(&mut self, index: usize, cause: Arc<str>) {
        let state = &self.metadata.init_states[index];
        if matches!(state.load(Ordering::Relaxed), SlotState::Failed) {
            return;
        }
        self.waiters.failures.push(SlotFailure { index, cause });
        state.fail();
        self.changed = true;
    }
}

impl Drop for HeapPublication<'_> {
    fn drop(&mut self) {
        // The waiter lock stays held through notification, preventing a waiter
        // from missing the transition between its state check and parking.
        if self.changed
            && let Some(cv) = self.metadata.init_waiters.cv.get()
        {
            cv.notify_all();
        }
    }
}

/// A heap's header once loaded: how to reopen its data, and where the lazily
/// parsed slot metadata starts within it. The dependencies the header names
/// are bound on the heap itself.
#[derive(Allocative)]
struct HeapHeaderState {
    recipe: Arc<dyn PagableDeserializerRecipe>,
    metadata_start: PagableCursor,
}

/// The owning `FrozenFrozenHeap`'s arena and the information needed to lazily
/// parse this heap's slot metadata from its data. A restored heap passes
/// through stages, each on first need: skeleton, header loaded, slot metadata
/// parsed, values restored one by one.
#[derive(Allocative)]
pub(crate) struct HeapDeserializationState {
    heap_id: HeapRefId,
    /// Scope state that owns cross-heap resolution for this heap.
    scope: Arc<StarlarkDeserScope>,
    /// Where the heap's data is, for a skeleton bound from a ref list without
    /// loading its header. `None` for a heap read in place.
    source: Option<DataKey>,
    /// The header, once loaded.
    header: OnceLock<HeapHeaderState>,
    /// Immutable pointer into the owning `FrozenFrozenHeap`'s arena.
    #[allocative(skip)]
    arena: NonNull<Arena<ChunkAllocator>>,
    /// Serializes allocation and sentinel publication against claims and walks.
    claims: Mutex<()>,
    /// Whether a claim has allocated since the serialization index was last
    /// published; see `refresh_serialization_index`.
    serialization_index_dirty: AtomicBool,
    /// Lazy: parsed on first `try_claim` / `value_count` call.
    metadata: OnceLock<HeapMetadata>,
}

// SAFETY: `arena` points into a heap-allocated `FrozenFrozenHeap` kept alive
// for the state's lifetime; `claims` serializes allocations and excludes walks
// until each new slot has its sentinel header and size metadata.
unsafe impl Sync for HeapDeserializationState {}
unsafe impl Send for HeapDeserializationState {}

impl HeapDeserializationState {
    /// # Safety
    /// `arena` must point to a `FrozenFrozenHeap.arena` whose containing
    /// `FrozenFrozenHeap` will be kept alive for at least
    /// as long as this `HeapDeserializationState`.
    pub(crate) unsafe fn new(
        scope: Arc<StarlarkDeserScope>,
        heap_id: HeapRefId,
        source: Option<DataKey>,
        arena: *const Arena<ChunkAllocator>,
    ) -> Self {
        Self {
            scope,
            heap_id,
            source,
            header: OnceLock::new(),
            // SAFETY: caller's contract — `arena` is a valid pointer.
            arena: unsafe { NonNull::new_unchecked(arena as *mut _) },
            claims: Mutex::new(()),
            serialization_index_dirty: AtomicBool::new(true),
            metadata: OnceLock::new(),
        }
    }

    pub(crate) fn unregister_heap(&self, heap_ptr: FrozenHeapPtr) {
        self.scope.unregister_heap(self.heap_id, heap_ptr);
    }

    /// Where the heap's data is, if it is still in storage.
    pub(crate) fn source(&self) -> Option<DataKey> {
        self.source
    }

    /// Whether the header is loaded: the heap's dependencies are bound and the
    /// recipe continuing into its slot metadata is retained. Slot metadata and
    /// values may still be unread.
    pub(crate) fn is_header_loaded(&self) -> bool {
        self.header.get().is_some()
    }

    /// Record the header as loaded. Two loaders of one header record the same
    /// thing; the heap, and its retained bytes, are counted once, for the load
    /// kept.
    pub(crate) fn set_header(
        &self,
        recipe: Arc<dyn PagableDeserializerRecipe>,
        metadata_start: PagableCursor,
    ) {
        let header = HeapHeaderState {
            recipe,
            metadata_start,
        };
        if self.header.set(header).is_err() {
            return;
        }
        // Counts headers loaded and retained, whether or not any value is ever
        // claimed - a dependency loaded for its own dependencies lands here too.
        if partial_deser_stats::enabled() {
            partial_deser_stats::add(&partial_deser_stats::HEAPS_LOADED, 1);
            if let Some(header) = self.header.get() {
                partial_deser_stats::add(
                    &partial_deser_stats::HEAP_RETAINED_BLOB_BYTES,
                    recipe_retained_data_len(&*header.recipe),
                );
            }
        }
    }

    fn header(&self) -> crate::Result<&HeapHeaderState> {
        self.header.get().ok_or_else(|| {
            anyhow::anyhow!(
                "heap {:?}: values requested before its header was loaded",
                self.heap_id
            )
            .into()
        })
    }

    /// Hold off claims while the guard lives: a claim allocates before writing
    /// its header, and a walk in that window would read an unwritten word.
    pub(crate) fn hold_claims(&self) -> MutexGuard<'_, ()> {
        self.claims.lock().expect("claim lock poisoned")
    }

    /// Size of the claimed slot whose payload starts at `payload`, if any.
    /// `try_claim` records this before writing the sentinel header.
    pub(crate) fn uninitialized_slot_size(&self, payload: *const ()) -> Option<ValueAllocSize> {
        let metadata = self.metadata.get()?;
        let index = *metadata
            .original_indices_by_payload
            .read()
            .expect("original index map lock poisoned")
            .get(&(payload as usize))?;
        let slot = metadata.slots.get(index as usize)?;
        Some(ValueAllocSize::new(AlignedSize::new_bytes(
            slot.alloc_size.get() as usize,
        )))
    }

    /// Number of values in this heap.
    pub(crate) fn value_count(&self, storage: &PagableStorageHandle) -> crate::Result<usize> {
        Ok(self.metadata(storage)?.slots.len())
    }

    /// Parse the metadata region from the recipe: `total_count`,
    /// `drop_value_count`, offset table, per-value metadata. Called
    /// once on first `metadata()`; subsequent calls hit `OnceLock`.
    fn parse_metadata(
        &self,
        storage: &PagableStorageHandle,
    ) -> crate::Result<(HeapMetadata, Option<partial_deser_stats::MetadataStats>)> {
        let header = self.header()?;
        let mut de = header.recipe.open(storage);
        // SAFETY: `metadata_start` was captured while loading this header; it is
        // a valid position in the recipe.
        unsafe { de.seek(header.metadata_start) };

        let total_count = u32::pagable_deserialize(&mut *de)? as usize;
        let drop_value_count = u32::pagable_deserialize(&mut *de)? as usize;
        let table_entry_count = total_count + 1;
        let mut offset_table = Vec::with_capacity(table_entry_count);
        for _ in 0..table_entry_count {
            let mut buf = [0u8; 8];
            for b in &mut buf {
                *b = u8::pagable_deserialize(&mut *de)?;
            }
            let stream_offset = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let arc_offset = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
            offset_table.push((stream_offset, arc_offset));
        }

        let mut slots: Vec<ValueDeserSlot> = Vec::with_capacity(total_count);
        for (i, &(stream_offset, arc_offset)) in offset_table.iter().take(total_count).enumerate() {
            let deser_type_id = DeserTypeId::pagable_deserialize(&mut *de)?;
            let vtable = lookup_vtable(deser_type_id)?;
            // A zero alloc_size is never valid (every value occupies at least its
            // header), so a zero here means a corrupt page-out stream.
            let alloc_size = NonZeroU32::new(u32::pagable_deserialize(&mut *de)?)
                .ok_or(PagableError::ZeroAllocSize { index: i })?;
            let bump_kind = if i < drop_value_count {
                BumpKind::Drop
            } else {
                BumpKind::NonDrop
            };
            slots.push(ValueDeserSlot::new(
                stream_offset,
                arc_offset,
                vtable,
                bump_kind,
                alloc_size,
            ));
        }
        // base_pos is the cursor right after per-value metadata — i.e. now.
        let base_pos = de.position();

        // Recorded by `metadata` only for the parse that is kept: two threads
        // can race to parse one heap, and counting here would count it twice.
        let stats = partial_deser_stats::enabled().then(|| partial_deser_stats::MetadataStats {
            retained_blob_bytes: recipe_retained_data_len(&*header.recipe),
            values: total_count as u64,
            // Walks every slot, so it is behind the same branch.
            value_alloc_bytes: slots.iter().map(|s| s.alloc_size.get() as u64).sum(),
            // The sentinel entry makes the value region's extent last minus
            // first offset. `saturating_sub`: these come off the stream, and a
            // diagnostic must not panic on a malformed table.
            value_serialized_bytes: match (offset_table.first(), offset_table.last()) {
                (Some(first), Some(last)) => last.0.saturating_sub(first.0) as u64,
                _ => 0,
            },
        });
        let init_states: Vec<AtomicSlotState> = (0..total_count)
            .map(|_| AtomicSlotState::not_started())
            .collect();
        Ok((
            HeapMetadata {
                slots,
                base_pos,
                init_states,
                original_indices_by_payload: RwLock::new(HashMap::new()),
                init_waiters: InitWaiters::new(),
            },
            stats,
        ))
    }

    /// Get parsed metadata, parsing on first call. Subsequent calls hit `OnceLock`.
    fn metadata(&self, storage: &PagableStorageHandle) -> crate::Result<&HeapMetadata> {
        if let Some(m) = self.metadata.get() {
            return Ok(m);
        }
        let (parsed, stats) = self.parse_metadata(storage)?;
        // `set` reports whether this parse is the one kept, so a heap is
        // counted once however many threads raced to parse it.
        if self.metadata.set(parsed).is_ok()
            && let Some(stats) = stats
        {
            stats.record();
        }
        Ok(self
            .metadata
            .get()
            .expect("metadata is populated: this thread set it, or lost to one that did"))
    }

    /// Return the header pointer only after transitive readiness is published.
    #[inline]
    fn ready_header_ptr(&self, index: usize) -> Option<*mut AValueHeader> {
        let m = self.metadata.get()?;
        m.init_states[index].load(Ordering::Acquire).ready_ptr()
    }

    /// Return the original recipe index for a claimed payload pointer.
    pub(crate) fn original_value_index(&self, raw_ptr: usize) -> Option<u32> {
        self.metadata
            .get()?
            .original_indices_by_payload
            .read()
            .expect("original index map lock poisoned")
            .get(&raw_ptr)
            .copied()
    }

    /// Walkers must not expose locally constructed values with unfinished edges.
    pub(crate) fn unreadable_slot_size(&self, payload: *const ()) -> Option<ValueAllocSize> {
        let index = self.original_value_index(payload as usize)? as usize;
        let m = self.metadata.get()?;
        if m.init_states[index]
            .load(Ordering::Acquire)
            .ready_ptr()
            .is_some()
        {
            return None;
        }
        Some(ValueAllocSize::new(AlignedSize::new_bytes(
            m.slots[index].alloc_size.get() as usize,
        )))
    }

    pub(crate) fn serialization_index_is_dirty(&self) -> bool {
        self.serialization_index_dirty.load(Ordering::Acquire)
    }

    pub(crate) fn refresh_serialization_index(
        &self,
        is_registered: impl Fn() -> bool,
        register: impl FnOnce(Vec<ChunkInfo>),
    ) {
        if !self.serialization_index_is_dirty() && is_registered() {
            return;
        }
        // SAFETY: `arena` remains valid for this state's lifetime.
        let arena = unsafe { self.arena.as_ref() };
        // Built and published under the claim hold, so the index is current
        // when it lands; the flag clears only then.
        arena.refresh_chunk_index(
            || !self.serialization_index_is_dirty() && is_registered(),
            |entries| {
                register(entries);
                self.serialization_index_dirty
                    .store(false, Ordering::Release);
            },
        );
    }

    /// Try to claim a slot for deserialization.
    ///
    /// Claims are serialized by the claim lock: the winner allocates the header
    /// and publishes its pointer into the slot's atomic *before* releasing the
    /// lock, so a claimed slot always carries its pointer and no reader ever has
    /// to wait for it to appear. This is why observing a started slot never blocks.
    ///
    /// The winner also records this slot's `payload address -> recipe index`
    /// mapping before publishing, so the wire identity of a lazily allocated
    /// value is in place for the whole time that value is reachable.
    ///
    /// On win, returns an armed guard with a sentinel-vtable allocation. The
    /// caller initializes its payload through the recipe, then consumes the
    /// guard with `publish` or `abort`. Dropping it also aborts the claim.
    /// On loss, returns the slot's terminal state or its in-progress deserialization pointer.
    fn try_claim(
        &self,
        value: HeapValueId,
        storage: &PagableStorageHandle,
    ) -> crate::Result<ClaimResult<'_>> {
        let index = value.value_index as usize;
        let m = self.metadata(storage)?;
        let state = &m.init_states[index];

        // Fast path: a started slot (done, failed, or in-progress) is fully
        // decodable without taking the lock.
        if let Some(result) = state.load(Ordering::Acquire).observed_claim_result() {
            return Ok(result);
        }

        // The claim lock serializes claims: its holder performs the not-started
        // -> in-progress transition and publishes the header pointer before
        // releasing, so an in-progress slot is never visible without its pointer.
        let claims_held = self.hold_claims();

        // Re-check under the lock; the slot may have been claimed since the load
        // above.
        if let Some(result) = state.load(Ordering::Acquire).observed_claim_result() {
            return Ok(result);
        }

        let slot = &m.slots[index];
        let abs_pos = PagableCursor {
            byte_pos: m.base_pos.byte_pos + slot.stream_offset as usize,
            arc_index: m.base_pos.arc_index + slot.arc_offset as usize,
        };
        let recipe_index = u32::try_from(index).expect("recipe index should fit in u32");
        // Lock-free index checks must see dirty before the arena can grow.
        self.serialization_index_dirty
            .store(true, Ordering::Release);
        // SAFETY: pointer valid for the state's lifetime; we hold the lock so
        // concurrent allocation is excluded.
        let header_ptr = unsafe {
            self.arena
                .as_ref()
                .alloc_raw_one(slot.bump_kind, slot.alloc_size)
        };
        let header_ptr = NonNull::new(header_ptr).expect("the arena never allocates at null");
        let raw_ptr = StarlarkValueRawPtr::new_header_ptr(header_ptr.as_ptr());
        // Indexed before the header is written, for walks that meet the sentinel.
        m.original_indices_by_payload
            .write()
            .expect("original index map lock poisoned")
            .insert(raw_ptr.ptr as usize, recipe_index);
        // SAFETY: sentinel vtable so any access before `starlark_deserialize`
        // would panic.
        unsafe {
            std::ptr::write(
                header_ptr.as_ptr(),
                AValueHeader(AValueVTable::uninitialized_sentinel()),
            );
        }
        state.publish_in_progress(header_ptr.as_ptr());
        let claim = SlotClaimGuard {
            state: self,
            value,
            claim: Some(DeserializeRecipe {
                index,
                abs_pos,
                vtable: slot.vtable,
                raw_ptr,
                header_ptr,
            }),
        };
        drop(claims_held);

        // On the winning claim only, so each value is counted once.
        if partial_deser_stats::enabled() {
            partial_deser_stats::add(&partial_deser_stats::CLAIMED_VALUES, 1);
            partial_deser_stats::add(
                &partial_deser_stats::CLAIMED_ALLOC_BYTES,
                slot.alloc_size.get() as u64,
            );
        }

        Ok(ClaimResult::Claimed(claim))
    }

    /// Construction waiters can store a completed payload's pointer; owning
    /// readers must also wait for its construction dependencies.
    fn wait_for_slot(
        &self,
        index: usize,
        storage: &PagableStorageHandle,
        until: WaitFor,
    ) -> crate::Result<SlotReadiness> {
        let m = self.metadata(storage)?;
        let state = &m.init_states[index];
        let cv = m.init_waiters.cv.get_or_init(Condvar::new);
        let mut guard = m
            .init_waiters
            .state
            .lock()
            .expect("init waiter lock poisoned");

        loop {
            match state.load(Ordering::Acquire) {
                SlotState::Ready(ptr) => return Ok(SlotReadiness::Ready(ptr)),
                SlotState::Failed => {
                    // Reconstructing the recorded error takes the same waiter lock.
                    drop(guard);
                    return Err(self.partial_deserialization_error(index as u32).into());
                }
                SlotState::Constructed(ptr) if matches!(until, WaitFor::Constructed) => {
                    return Ok(SlotReadiness::Pending(ptr));
                }
                SlotState::NotStarted | SlotState::InProgress(_) | SlotState::Constructed(_) => {}
            }

            #[cfg(all(test, feature = "pagable"))]
            if guard.wait_started.as_ref().is_some_and(|(thread, _)| {
                thread.is_none_or(|thread| thread == std::thread::current().id())
            }) {
                let (_, started) = guard.wait_started.take().expect("matching test waiter");
                // Terminal publication cannot overtake this notification before
                // `cv.wait` releases the waiter lock.
                let _ = started.send(());
            }
            guard = cv.wait(guard).expect("init waiter lock poisoned");
        }
    }

    #[cfg(all(test, feature = "pagable"))]
    pub(crate) fn notify_on_wait_for_test(&self, thread: Option<ThreadId>, started: Sender<()>) {
        self.metadata
            .get()
            .expect("test heap metadata must already be read")
            .init_waiters
            .state
            .lock()
            .expect("init waiter lock poisoned")
            .wait_started = Some((thread, started));
    }

    /// Complete local construction after `write_vtable_to_header`. A cycle's
    /// owning readers stay blocked until its remaining writers also complete.
    fn finalize_claim(
        &self,
        value: HeapValueId,
        claim: DeserializeRecipe,
    ) -> crate::Result<SlotReadiness> {
        let finalized = self
            .publication()
            .try_finalize(claim.index, claim.header_ptr.as_ptr());
        if finalized {
            return Ok(SlotReadiness::Ready(claim.header_ptr.as_ptr()));
        }
        // A removed dependency can own the last heap reference. Destructors
        // must not run under the readiness lock.
        let mut retired = Vec::new();
        let result = self
            .scope
            .wait_graph
            .readiness
            .lock()
            .expect("readiness lock poisoned")
            .complete(value, self, &mut retired);
        drop(retired);
        result
    }

    /// Publish the claimed slot as failed, with `error` as the cause every
    /// later reader of the slot sees.
    #[cold]
    fn abort_claim(&self, value: HeapValueId, claim: DeserializeRecipe, error: &crate::Error) {
        let cause = Arc::<str>::from(format!("{error:#}"));
        let mut retired = Vec::new();
        let mut readiness = self
            .scope
            .wait_graph
            .readiness
            .lock()
            .expect("readiness lock poisoned");
        self.fail_slot(claim.index, cause.dupe());
        readiness.fail(value, cause, &mut retired);
        drop(readiness);
        drop(retired);
    }

    fn fail_slot(&self, index: usize, cause: Arc<str>) {
        self.publication().fail(index, cause);
    }

    /// Reconstruct the typed error for a slot whose original deserializer
    /// already failed.
    #[cold]
    fn partial_deserialization_error(&self, value_index: u32) -> PagableError {
        let m = self
            .metadata
            .get()
            .expect("failed slot must have parsed metadata");
        let index = value_index as usize;
        let cause = m
            .init_waiters
            .state
            .lock()
            .expect("init waiter lock poisoned")
            .failures
            .iter()
            .find(|failure| failure.index == index)
            .map(|failure| failure.cause.dupe())
            .unwrap_or_else(|| Arc::from("original deserialization error was not recorded"));

        PagableError::PartialDeserializationFailed {
            heap_id: self.heap_id,
            value_index,
            value_type: m.slots[index].vtable.type_name,
            cause,
        }
    }

    fn publication(&self) -> HeapPublication<'_> {
        let metadata = self
            .metadata
            .get()
            .expect("publication requires parsed metadata");
        let waiters = metadata
            .init_waiters
            .state
            .lock()
            .expect("init waiter lock poisoned");
        HeapPublication {
            metadata,
            waiters,
            changed: false,
        }
    }
}

/// The live heap for each heap identity, shared by every page-in over one
/// storage.
///
/// A [`HeapRefId`] names one heap's content, so which root page-in first bound
/// an allocation does not matter to any other: a pointer resolves into the
/// same heap whichever root reads it. One map per storage keeps a binding per
/// live heap; one map per root would copy each root's whole dependency closure.
#[derive(Allocative)]
pub(crate) struct StarlarkHeapBindings {
    bindings: DashMap<HeapRefId, WeakFrozenHeapRef>,
    registrations_until_prune: AtomicUsize,
}

const MIN_HEAP_BINDING_PRUNE_INTERVAL: usize = 64;

impl Default for StarlarkHeapBindings {
    fn default() -> Self {
        Self {
            bindings: DashMap::default(),
            registrations_until_prune: AtomicUsize::new(MIN_HEAP_BINDING_PRUNE_INTERVAL),
        }
    }
}

impl StarlarkHeapBindings {
    fn maybe_prune_expired(&self) {
        // Native heaps have no deserialization state to unregister them on drop.
        // Zero reserves the sweep for one caller while other registrations proceed.
        if self.registrations_until_prune.try_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |remaining| remaining.checked_sub(1),
        ) != Ok(1)
        {
            return;
        }

        // Upgrading here could drop the last strong reference under a shard lock;
        // restored heap destruction re-enters this map to unregister the heap.
        self.bindings.retain(|_, heap| !heap.is_expired());
        // Retain scans capacity, so the next interval can track live entries only
        // if excess capacity from expired entries is also reclaimed.
        self.bindings.shrink_to_fit();
        self.registrations_until_prune.store(
            self.bindings.len().max(MIN_HEAP_BINDING_PRUNE_INTERVAL),
            Ordering::Relaxed,
        );
    }
}

impl StorageState for StarlarkHeapBindings {}

/// What all Starlark deserialization over one storage shares.
#[derive(Allocative)]
pub(crate) struct StarlarkDeserScope {
    heap_bindings: Arc<StarlarkHeapBindings>,
    defer_field_reads: bool,
    #[allocative(skip)]
    wait_graph: Arc<StarlarkDeserWaitGraph>,
}

/// Marks a storage whose deferred Starlark fields materialize only when read.
/// Install before deserializing any values; absent, fields are read eagerly.
pub struct DeferredFieldReadsEnabled;

impl StorageState for DeferredFieldReadsEnabled {}

/// Records a deferred read that failed on `heap` in the storage's
/// [`DeferredReadFailures`]. Only deferred reads report: an eager page-in
/// that fails falls back to recomputing.
#[cfg(fbcode_build)]
fn record_deferred_read_failure(
    storage_context: &StorageContext,
    heap: &FrozenHeapArc,
    error: &crate::Error,
) {
    let heap = heap
        .name()
        .map_or_else(|| "<unnamed heap>".to_owned(), ToString::to_string);
    storage_context
        .get_or_init(DeferredReadFailures::default)
        .record_error(format_args!("heap `{heap}`"), error.inner());
}

/// The published `pagable` predates `DeferredReadFailures`; the OSS build has
/// no host that reads it either. Compiled only where `with_deferred_values` is.
#[cfg(all(not(fbcode_build), test, feature = "pagable"))]
fn record_deferred_read_failure(
    _storage_context: &StorageContext,
    _heap: &FrozenHeapArc,
    _error: &crate::Error,
) {
}

impl StorageState for StarlarkDeserScope {}

/// Estimate memory retained by cached Starlark heap deserialization state.
///
/// All heap states are visited with one builder so shared scopes and serialized
/// recipe data are counted once. The heap arenas themselves are not followed.
pub fn starlark_deserialization_state_retained_bytes(storage: &PagableStorageHandle) -> usize {
    cached_heap_deserialization_state_retained_bytes(storage)
}

/// `recipe.retained_data_len()`, compiled out where the OSS build's published
/// `pagable` predates the method. Unreachable there: `enabled()` is false
/// without `fbcode_build`. Remove with that check once a release has it.
fn recipe_retained_data_len(recipe: &dyn PagableDeserializerRecipe) -> u64 {
    #[cfg(fbcode_build)]
    return recipe.retained_data_len() as u64;
    #[cfg(not(fbcode_build))]
    {
        let _ = recipe;
        unreachable!("counting is disabled outside fbcode_build")
    }
}

/// Process-wide counters contrasting what a page-in must load with what it
/// uses. A value is only reachable through its heap, so claiming one value
/// pays for the whole heap's blob and slot table: `heap_*` is that cost,
/// `claimed_*` the part actually asked for.
mod partial_deser_stats {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;

    pub(super) static HEAPS_LOADED: AtomicU64 = AtomicU64::new(0);
    pub(super) static HEAP_RETAINED_BLOB_BYTES: AtomicU64 = AtomicU64::new(0);
    pub(super) static HEAPS_WITH_METADATA: AtomicU64 = AtomicU64::new(0);
    pub(super) static USED_HEAP_RETAINED_BLOB_BYTES: AtomicU64 = AtomicU64::new(0);
    pub(super) static HEAP_VALUE_SERIALIZED_BYTES: AtomicU64 = AtomicU64::new(0);
    pub(super) static HEAP_VALUES: AtomicU64 = AtomicU64::new(0);
    pub(super) static HEAP_VALUE_ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
    pub(super) static CLAIMED_VALUES: AtomicU64 = AtomicU64::new(0);
    pub(super) static CLAIMED_ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);

    /// Off unless `BUCK2_STARLARK_PARTIAL_DESER_STATS` is set: the claim site
    /// runs millions of times per page-in and shared counters there are a
    /// contended cache line. Always on under `cfg(test)` in the fbcode build —
    /// the `fbcode_build` gate below runs first — so the counting paths stay
    /// exercised there; the counters are process-global and never reset, so
    /// tests must not assert on absolute values. Callers branch on this
    /// before computing anything a counter needs, not just before storing it.
    pub(super) fn enabled() -> bool {
        // fbcode-only until a `pagable` release with `retained_data_len` is
        // published (see `recipe_retained_data_len`).
        if !cfg!(fbcode_build) {
            return false;
        }
        if cfg!(test) {
            return true;
        }
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("BUCK2_STARLARK_PARTIAL_DESER_STATS").is_some())
    }

    /// What one heap's metadata parse contributes, held until the parse is
    /// known to be the one kept - see `HeapDeserializationState::metadata`.
    pub(super) struct MetadataStats {
        pub(super) retained_blob_bytes: u64,
        pub(super) values: u64,
        pub(super) value_alloc_bytes: u64,
        pub(super) value_serialized_bytes: u64,
    }

    impl MetadataStats {
        pub(super) fn record(&self) {
            add(&HEAPS_WITH_METADATA, 1);
            add(&USED_HEAP_RETAINED_BLOB_BYTES, self.retained_blob_bytes);
            add(&HEAP_VALUES, self.values);
            add(&HEAP_VALUE_ALLOC_BYTES, self.value_alloc_bytes);
            add(&HEAP_VALUE_SERIALIZED_BYTES, self.value_serialized_bytes);
        }
    }

    /// Relaxed throughout: these are monotonic counters read as a gauge, never
    /// used to order other memory.
    pub(super) fn add(counter: &AtomicU64, n: u64) {
        counter.fetch_add(n, Ordering::Relaxed);
    }

    pub(super) fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

/// A snapshot of [`starlark_partial_deser_stats`].
#[derive(Debug, Clone, Copy, Dupe, Default, PartialEq, Eq)]
pub struct PartialDeserStats {
    /// Heaps restored from storage to the header-loaded stage: dependencies
    /// bound and the data's recipe retained, slot metadata and values still
    /// lazy. A skeleton bound without a load is not counted; a heap loaded
    /// only as another heap's dependency is.
    pub heaps_loaded: u64,
    /// Serialized bytes those blobs held so any value in them could still be
    /// claimed later. Cumulative like the rest: bytes are not subtracted when
    /// a heap is dropped. Recipes and heap blobs are one-to-one today; a
    /// shared blob would be counted once per heap.
    pub heap_retained_blob_bytes: u64,
    /// Of `heaps_loaded`, those whose slot table was parsed — a resolve
    /// reached them. The remainder were loaded but never used.
    pub heaps_with_metadata: u64,
    /// The `heap_retained_blob_bytes` belonging to those heaps.
    pub used_heap_retained_blob_bytes: u64,
    /// Serialized value-region bytes of the heaps in `heaps_with_metadata`.
    pub heap_value_serialized_bytes: u64,
    /// Values contained in those heaps.
    pub heap_values: u64,
    /// In-memory bytes those heaps would occupy if every value were materialized.
    pub heap_value_alloc_bytes: u64,
    /// Values actually claimed for deserialization.
    pub claimed_values: u64,
    /// In-memory bytes of the claimed values — the part of
    /// `heap_value_alloc_bytes` actually built.
    pub claimed_alloc_bytes: u64,
}

/// Process-wide partial-deserialization counters since daemon start, or `None`
/// if counting is off, so callers report "not measured" rather than zeros.
pub fn starlark_partial_deser_stats() -> Option<PartialDeserStats> {
    use partial_deser_stats as s;
    if !s::enabled() {
        return None;
    }
    Some(PartialDeserStats {
        heaps_loaded: s::get(&s::HEAPS_LOADED),
        heap_retained_blob_bytes: s::get(&s::HEAP_RETAINED_BLOB_BYTES),
        heaps_with_metadata: s::get(&s::HEAPS_WITH_METADATA),
        used_heap_retained_blob_bytes: s::get(&s::USED_HEAP_RETAINED_BLOB_BYTES),
        heap_value_serialized_bytes: s::get(&s::HEAP_VALUE_SERIALIZED_BYTES),
        heap_values: s::get(&s::HEAP_VALUES),
        heap_value_alloc_bytes: s::get(&s::HEAP_VALUE_ALLOC_BYTES),
        claimed_values: s::get(&s::CLAIMED_VALUES),
        claimed_alloc_bytes: s::get(&s::CLAIMED_ALLOC_BYTES),
    })
}

/// Exact process-local value identity used only while a claim or wait guard is
/// active. The caller retains the owning heap, so `heap_ptr` cannot be reused
/// while this identity is present in the graph.
#[derive(Debug, Clone, Copy, Dupe, Eq, PartialEq, Hash)]
pub(crate) struct HeapValueId {
    heap_ptr: FrozenHeapPtr,
    value_index: u32,
}

/// What a thread in the wait graph is constructing, or blocked waiting for.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, From)]
pub(crate) enum DeserWaitKey {
    /// A heap value being deserialized into its slot. Claimed by the thread
    /// that won the slot's claim; waited on by a thread that needs the value
    /// before that claim is finalized.
    Value(HeapValueId),
    /// A deferred field being resolved, identified by the address of its word
    /// in the owning frozen heap, which its readers keep alive. Claimed by the
    /// resolver that took the field out of its pending state; waited on by
    /// later readers of the same field until the resolver publishes. A field
    /// has no readiness group, so a cycle walk does not expand writers for it.
    Field(usize),
}

/// Storage-scoped wait-for graph for detecting cyclic-deserialization deadlocks.
///
/// The Arc cache can share a partially deserialized heap between root page-in
/// scopes, so every root using the same storage must coordinate through one
/// graph. Exact heap pointers keep unrelated same-name heaps independent.
///
/// Keyed by [`ThreadId`], which identifies one in-flight deserialization only
/// because the deserialize path is synchronous and blocks by parking the OS
/// thread (`wait_for_slot`).
///
/// ATTENTION: if deserialization ever becomes async, one thread could drive two
/// at once and corrupt these keys — key by a per-deserialization token instead.
#[derive(Default)]
pub(crate) struct StarlarkDeserWaitGraph {
    /// Maps an exact heap value or deferred field to its deserializing thread.
    ///
    /// Written on every claim and release; read only when a thread is about to
    /// block, to walk the wait-for chain.
    claimers: DashMap<DeserWaitKey, ThreadId>,
    /// Maps a thread to the exact heap value or field it is blocked waiting on.
    ///
    /// A thread about to block inserts its edge here and then follows the
    /// chain - the value's claimer, what that thread waits on, its claimer,
    /// and so on - looking for itself, all under this lock
    /// (`begin_wait_and_check_cycle`). Holding the lock across both steps is
    /// what makes the check sound: two threads about to wait on each other
    /// cannot both walk before either's edge is in, so one of them sees the
    /// cycle. `claimers` is not covered by the lock and can change during a
    /// walk, but every edge of a real deadlock belongs to a thread already
    /// blocked here, and those edges cannot change.
    waiters: Mutex<HashMap<ThreadId, DeserWaitKey>>,
    /// Empty on acyclic restores; contains only unfinished cycle dependencies.
    readiness: Mutex<ReadinessGraph>,
}

impl StorageState for StarlarkDeserWaitGraph {}

impl StarlarkDeserWaitGraph {
    /// Threads blocked on a claimed value or field right now.
    #[cfg(all(test, fbcode_build))]
    pub(crate) fn waiting_threads(&self) -> usize {
        self.lock_waiters().len()
    }

    fn lock_waiters(&self) -> MutexGuard<'_, HashMap<ThreadId, DeserWaitKey>> {
        self.waiters.lock().expect("wait-for graph lock poisoned")
    }

    /// Record that `thread` has claimed `value` for deserialization. The
    /// returned guard removes the `claimers` edge on drop, so every exit path
    /// from a claimed deserialization unwinds it exactly once.
    pub(crate) fn claim(
        self: &Arc<Self>,
        value: impl Into<DeserWaitKey>,
        thread: ThreadId,
    ) -> ClaimGuard {
        let value = value.into();
        self.claimers.insert(value, thread);
        ClaimGuard {
            graph: self.dupe(),
            value,
        }
    }

    /// Record that `thread` is about to wait on `value` and, atomically with that
    /// insert, report whether waiting would deadlock (a wait-for cycle). Hold the
    /// returned guard for the whole wait so other threads' cycle checks see it.
    pub(crate) fn begin_wait_and_check_cycle(
        self: &Arc<Self>,
        thread: ThreadId,
        value: impl Into<DeserWaitKey>,
    ) -> (WaitGuard, bool) {
        let value = value.into();
        let mut waiters = self.lock_waiters();
        waiters.insert(thread, value);
        let cycle = self.has_cycle(&waiters, thread, value);
        drop(waiters);
        (
            WaitGuard {
                graph: self.dupe(),
                thread,
            },
            cycle,
        )
    }

    /// True if blocking on `start_value` would deadlock: the wait-for chain
    /// leads back to `my_thread` (covers same-thread re-entry too).
    fn has_cycle(
        &self,
        waiters: &HashMap<ThreadId, DeserWaitKey>,
        my_thread: ThreadId,
        start_value: DeserWaitKey,
    ) -> bool {
        // Bounded by `waiters`: each step lands on a distinct waiting thread.
        // `claimers` is not frozen by this lock, but a real deadlock's edges
        // all belong to blocked threads and cannot move mid-walk, and a chain
        // that churn cuts or extends passes through a live thread and is not
        // one. The caller's own wait edge must already be in, as
        // `begin_wait_and_check_cycle` ensures.
        debug_assert!(
            waiters.contains_key(&my_thread),
            "cycle check requires the caller's wait edge to be present"
        );
        let mut current = start_value;
        for _ in 0..waiters.len() {
            let Some(claimer) = self.claimers.get(&current).map(|c| *c) else {
                return self.has_cycle_through_readiness(waiters, my_thread, current);
            };
            if claimer == my_thread {
                return true;
            }
            let Some(&waiting_for) = waiters.get(&claimer) else {
                return false;
            };
            current = waiting_for;
        }
        false
    }

    /// A locally completed value can wait on several unfinished constructors.
    /// Ordinary construction-only wait chains need neither this traversal nor
    /// its temporary visited set.
    fn has_cycle_through_readiness(
        &self,
        waiters: &HashMap<ThreadId, DeserWaitKey>,
        my_thread: ThreadId,
        start: DeserWaitKey,
    ) -> bool {
        let readiness = self.readiness.lock().expect("readiness lock poisoned");
        // `has_cycle` hands over a `start` with no claimer, so the first step
        // below expands its readiness writers; a field has none.
        let mut pending = vec![start];
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            let Some(claimer) = self.claimers.get(&current).map(|c| *c) else {
                // Locally completed claims no longer have a constructing thread.
                // A readiness waiter depends on the group's remaining writers.
                if let DeserWaitKey::Value(value) = current {
                    pending.extend(
                        readiness
                            .pending_writers(value)
                            .into_iter()
                            .map(DeserWaitKey::Value),
                    );
                }
                continue;
            };
            if claimer == my_thread {
                return true;
            }
            if let Some(&waiting_for) = waiters.get(&claimer) {
                pending.push(waiting_for);
            }
        }
        false
    }
}

/// Clears a claim's `claimers` edge on drop, so every exit path from a claimed
/// deserialization unwinds it exactly once.
pub(crate) struct ClaimGuard {
    graph: Arc<StarlarkDeserWaitGraph>,
    value: DeserWaitKey,
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        self.graph.claimers.remove(&self.value);
    }
}

/// Clears this thread's `waiters` edge on drop.
pub(crate) struct WaitGuard {
    graph: Arc<StarlarkDeserWaitGraph>,
    thread: ThreadId,
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        self.graph.lock_waiters().remove(&self.thread);
    }
}

/// Describe a heap-identity collision. Cold because naming the heap allocates a
/// `String` that the (overwhelmingly common) non-conflicting path never needs.
#[cold]
fn conflicting_heap_binding(
    heap_id: HeapRefId,
    bound: &FrozenHeapArc,
    bound_heap_ptr: FrozenHeapPtr,
    conflicting_origin: Option<HeapAllocationOrigin>,
    conflicting_heap_ptr: FrozenHeapPtr,
) -> PagableError {
    PagableError::ConflictingHeapBinding {
        heap_id,
        heap_name: bound
            .name()
            .map_or_else(|| "<unnamed>".to_owned(), |name| name.to_string()),
        bound_heap_ptr: bound_heap_ptr.addr(),
        bound_origin: bound.allocation_origin(),
        conflicting_heap_ptr: conflicting_heap_ptr.addr(),
        conflicting_origin,
    }
}

impl StarlarkDeserScope {
    pub(crate) fn new(
        heap_bindings: Arc<StarlarkHeapBindings>,
        wait_graph: Arc<StarlarkDeserWaitGraph>,
    ) -> Self {
        Self {
            heap_bindings,
            defer_field_reads: false,
            wait_graph,
        }
    }

    pub(crate) fn wait_graph(&self) -> &Arc<StarlarkDeserWaitGraph> {
        &self.wait_graph
    }

    /// Register a heap for cross-heap value resolution.
    pub(crate) fn register_heap(
        &self,
        heap_id: HeapRefId,
        heap: WeakFrozenHeapRef,
    ) -> Result<(), PagableError> {
        let heap_ptr = heap.heap_ptr();
        match self.heap_bindings.bindings.entry(heap_id) {
            Entry::Vacant(entry) => {
                entry.insert(heap);
            }
            Entry::Occupied(mut entry) => {
                if entry.get().heap_ptr() == heap_ptr {
                    return Ok(());
                }
                if let Some(bound) = entry.get().upgrade() {
                    return Err(conflicting_heap_binding(
                        heap_id,
                        &bound,
                        entry.get().heap_ptr(),
                        heap.upgrade()
                            .as_ref()
                            .map(FrozenHeapArc::allocation_origin),
                        heap_ptr,
                    ));
                }
                entry.insert(heap);
            }
        }
        self.heap_bindings.maybe_prune_expired();
        Ok(())
    }

    pub(crate) fn is_heap_bound(
        &self,
        heap_id: HeapRefId,
        heap: &FrozenHeapArc,
    ) -> Result<bool, PagableError> {
        let heap_ptr = heap
            .downgrade()
            .expect("a heap being bound must have an allocation")
            .heap_ptr();
        let Some(entry) = self.heap_bindings.bindings.get(&heap_id) else {
            return Ok(false);
        };
        if entry.heap_ptr() == heap_ptr {
            return Ok(true);
        }
        if let Some(bound) = entry.upgrade() {
            return Err(conflicting_heap_binding(
                heap_id,
                &bound,
                entry.heap_ptr(),
                Some(heap.allocation_origin()),
                heap_ptr,
            ));
        }
        Ok(false)
    }

    pub(crate) fn unregister_heap(&self, heap_id: HeapRefId, heap_ptr: FrozenHeapPtr) {
        if let Entry::Occupied(entry) = self.heap_bindings.bindings.entry(heap_id)
            && entry.get().heap_ptr() == heap_ptr
        {
            entry.remove();
        }
    }

    pub(crate) fn get_heap(&self, heap_id: &HeapRefId) -> Option<FrozenHeapArc> {
        self.heap_bindings
            .bindings
            .get(heap_id)
            .and_then(|heap| heap.upgrade())
    }

    /// Bound heaps whose header is not loaded, so whose dependencies are not
    /// bound yet.
    fn heaps_without_header(&self) -> Vec<FrozenHeapArc> {
        self.heap_bindings
            .bindings
            .iter()
            .filter_map(|entry| entry.value().upgrade())
            .filter(|heap| !heap.is_header_loaded())
            .collect()
    }
}

/// Bind a heap a pointer names but nothing has bound yet.
///
/// Try [`StarlarkHeapKeyIndex`], then load headers breadth-first through `origin`'s
/// serialized refs. Targets outside that closure, such as relocated values,
/// require the index.
///
/// `origin` retains index hits directly and BFS hits through their ref path.
/// Without an origin, search all unread heaps and rely on the arc cache for
/// retention.
///
/// [`StarlarkHeapKeyIndex`]: crate::values::layout::heap::sealed::heap_key_index::StarlarkHeapKeyIndex
#[cold]
fn resolve_missing_heap(
    scope: &Arc<StarlarkDeserScope>,
    storage: &PagableStorageHandle,
    origin: Option<&FrozenHeapArc>,
    heap_id: HeapRefId,
) -> crate::Result<FrozenHeapArc> {
    if let Some(found) = load_and_bind_heap_by_id(scope, storage, heap_id)? {
        if let Some(origin) = origin {
            origin.retain_dependency(&found);
        }
        return Ok(found);
    }

    let mut queue: VecDeque<FrozenHeapArc> = match origin {
        Some(origin) => VecDeque::from([origin.dupe()]),
        None => scope.heaps_without_header().into(),
    };
    let mut seen: HashSet<FrozenHeapPtr> = HashSet::new();
    while let Some(heap) = queue.pop_front() {
        let Some(ptr) = heap.downgrade().map(|weak| weak.heap_ptr()) else {
            continue;
        };
        if !seen.insert(ptr) {
            continue;
        }
        if !heap.is_header_loaded() {
            heap.ensure_header_loaded(scope, storage)?;
            if let Some(found) = scope.get_heap(&heap_id) {
                return Ok(found);
            }
        }
        // Within the serialized closure, retained edges only shortcut ref paths.
        // Targets outside it require the index, whose entries never expire.
        // Restart drops retained edges too, so serialized refs suffice here.
        queue.extend(heap.refs_slice().iter().map(|dep| dep.heap_arc().dupe()));
    }
    Err(PagableError::HeapNotBound { heap_id }.into())
}

/// Concrete implementation of StarlarkDeserializeContext.
///
/// Wraps a `PagableDeserializer` and a shared `StarlarkDeserScope` to
/// resolve value references during deserialization.
pub(crate) struct StarlarkDeserializerImpl<'a, 'de, 'fv> {
    pagable: &'a mut dyn PagableDeserializer<'de>,
    /// Shared registry of per-heap deserialization state. Cross-heap pointer
    /// resolution looks up the target heap by `heap_id` here.
    scope: Arc<StarlarkDeserScope>,
    /// The heap whose values this context deserializes, when known: where a
    /// pointer into a heap not bound yet is looked for, and what retains it
    /// once found (see `resolve_missing_heap`).
    origin: Option<FrozenHeapArc>,
    /// The brand this context deserializes at; see [`recover_from_pagable`](Self::recover_from_pagable).
    brand: PhantomData<Value<'fv>>,
}

impl<'de> StarlarkDeserializerImpl<'_, 'de, '_> {
    /// Recover a `StarlarkDeserializerImpl` after a hop through a pagable-only
    /// boundary (typically `serialize_arc` / `deserialize_arc`) and run `f` with it. All heap
    /// state is reachable via the storage's `StarlarkDeserScope`.
    ///
    /// The context deserializes at a brand of its own, introduced here for `f` alone. The brand
    /// stands for the heap whose data `deserializer` is positioned in: the heap of the value
    /// being materialized, or, for the owning carriers (`OwnedFrozen`, `Globals`), the heap that
    /// was just deserialized ahead of the root value. Every value the context hands out was
    /// serialized from that heap, and a value in a frozen heap only ever points into that heap
    /// or into a heap it references (that is what the brand it was allocated at guaranteed), so
    /// the brand is honest for cross-heap pointers too. Being closure-introduced, it names
    /// nothing outside `f`; the callers that carry a result out of `f` are the framework itself,
    /// which either writes it into that heap (the `AValue` vtable, see `AValueFrozen`) or pairs
    /// it with the heap's owner (`OwnedFrozen::unchecked_new`).
    pub(crate) fn recover_from_pagable<R>(
        deserializer: &mut dyn PagableDeserializer<'de>,
        f: impl for<'fv> FnOnce(&mut StarlarkDeserializerImpl<'_, 'de, 'fv>) -> R,
    ) -> R {
        Self::recover_from_pagable_impl(deserializer, None, f)
    }

    /// [`recover_from_pagable`](Self::recover_from_pagable) for a context
    /// whose values are known to live in `origin`. This lets references to
    /// unbound heaps be resolved and retained on `origin`'s behalf.
    /// Independently owning results must instead use
    /// [`recover_root_from_pagable_in`](Self::recover_root_from_pagable_in).
    pub(crate) fn recover_from_pagable_in<R>(
        deserializer: &mut dyn PagableDeserializer<'de>,
        origin: &FrozenHeapArc,
        f: impl for<'fv> FnOnce(&mut StarlarkDeserializerImpl<'_, 'de, 'fv>) -> R,
    ) -> R {
        Self::recover_from_pagable_impl(deserializer, Some(origin.dupe()), f)
    }

    /// Owning results can escape independently of an enclosing constructor.
    /// They must wait for transitive readiness even inside a pagable Arc body.
    pub(crate) fn recover_root_from_pagable_in<R>(
        deserializer: &mut dyn PagableDeserializer<'de>,
        origin: &FrozenHeapArc,
        f: impl for<'fv> FnOnce(&mut StarlarkDeserializerImpl<'_, 'de, 'fv>) -> R,
    ) -> R {
        let _root = ActiveClaim::root();
        Self::recover_from_pagable_in(deserializer, origin, f)
    }

    fn recover_from_pagable_impl<R>(
        deserializer: &mut dyn PagableDeserializer<'de>,
        origin: Option<FrozenHeapArc>,
        f: impl for<'fv> FnOnce(&mut StarlarkDeserializerImpl<'_, 'de, 'fv>) -> R,
    ) -> R {
        let scope = Self::get_or_create_scope(deserializer);
        f(&mut StarlarkDeserializerImpl {
            pagable: deserializer,
            scope,
            origin,
            brand: PhantomData,
        })
    }

    /// Get or create the storage's Starlark scope. Also the point at which the
    /// storage starts indexing heaps by identity, ahead of any heap it binds.
    pub(crate) fn get_or_create_scope(
        deserializer: &mut dyn PagableDeserializer<'_>,
    ) -> Arc<StarlarkDeserScope> {
        let storage_context = deserializer.storage_context();
        if let Some(scope) = storage_context.get::<StarlarkDeserScope>() {
            return scope;
        }
        register_heap_key_index(storage_context);
        // Resolved before the scope's initializer, which runs under the
        // context registry's shard lock: these lookups go through the same
        // registry, and a `TypeId` sharing the scope's shard would wait on
        // the lock this thread holds.
        let heap_bindings = storage_context.get_or_init(StarlarkHeapBindings::default);
        let wait_graph = storage_context.get_or_init(StarlarkDeserWaitGraph::default);
        let defer_field_reads = storage_context.get::<DeferredFieldReadsEnabled>().is_some();
        storage_context.get_or_init(|| {
            let mut scope = StarlarkDeserScope::new(heap_bindings, wait_graph);
            scope.defer_field_reads = defer_field_reads;
            scope
        })
    }
}

impl<'de, 'fv> StarlarkDeserializeContext<'de, 'fv> for StarlarkDeserializerImpl<'_, 'de, 'fv> {
    fn pagable(&mut self) -> &mut dyn PagableDeserializer<'de> {
        self.pagable
    }

    fn deserialize_value(&mut self) -> crate::Result<Value<'fv>> {
        let serialized = SerializedFrozenValue::pagable_deserialize(self.pagable)?;
        let storage = self.pagable.storage();
        StarlarkValueResolver {
            scope: &self.scope,
            storage: &storage,
            #[cfg(not(fbcode_build))]
            storage_context: self.pagable.storage_context(),
            origin: self.origin.as_ref(),
            brand: PhantomData,
        }
        .resolve(&serialized)
    }

    #[cfg(fbcode_build)]
    fn deferred_read_context(&mut self) -> Option<DeferredReadContext<'fv>> {
        if !self.scope.defer_field_reads {
            return None;
        }
        // SAFETY: this context's brand names `origin`; its scope and storage
        // belong to this same deserialization.
        Some(unsafe {
            DeferredReadContext::new(
                self.scope.dupe(),
                self.pagable.storage(),
                self.origin.as_ref()?.downgrade()?,
            )
        })
    }
}

struct StarlarkValueResolver<'a, 'fv> {
    scope: &'a Arc<StarlarkDeserScope>,
    storage: &'a PagableStorageHandle,
    #[cfg(not(fbcode_build))]
    storage_context: &'a StorageContext,
    origin: Option<&'a FrozenHeapArc>,
    brand: PhantomData<Value<'fv>>,
}

/// Borrowed context for resolving a field against its owning heap.
#[cfg(any(fbcode_build, all(test, feature = "pagable")))]
pub(crate) struct DeferredResolveContext<'a> {
    pub(crate) scope: &'a Arc<StarlarkDeserScope>,
    pub(crate) storage: &'a PagableStorageHandle,
    #[cfg(not(fbcode_build))]
    pub(crate) storage_context: &'a StorageContext,
    pub(crate) origin: &'a FrozenHeapArc,
}

/// Resolve saved pointers at a fresh brand, retaining their target heaps on
/// the context's origin. The deferred field's adapter restores its original brand.
#[cfg(any(fbcode_build, all(test, feature = "pagable")))]
pub(crate) fn with_deferred_values<R>(
    refs: &[SerializedFrozenValue],
    context: DeferredResolveContext<'_>,
    f: impl for<'fv> FnOnce(&mut dyn Iterator<Item = crate::Result<Value<'fv>>>) -> R,
) -> R {
    // The callback can inspect and publish these values before its caller's
    // constructor finishes, so construction-only references must not escape.
    let _root = ActiveClaim::root();
    #[cfg(fbcode_build)]
    let storage_context = context.storage.storage_context();
    #[cfg(not(fbcode_build))]
    let storage_context = context.storage_context;
    let mut resolver = StarlarkValueResolver {
        scope: context.scope,
        storage: context.storage,
        #[cfg(not(fbcode_build))]
        storage_context: context.storage_context,
        origin: Some(context.origin),
        brand: PhantomData,
    };
    f(&mut refs.iter().map(|value| {
        resolver
            .resolve(value)
            .inspect_err(|error| {
                if matches!(value, SerializedFrozenValue::HeapPtr { .. }) {
                    record_deferred_read_failure(storage_context, context.origin, error);
                }
            })
            .map_err(crate::Error::into_deferred_read_error)
    }))
}

impl<'fv> StarlarkValueResolver<'_, 'fv> {
    fn resolve(&mut self, serialized: &SerializedFrozenValue) -> crate::Result<Value<'fv>> {
        match serialized {
            SerializedFrozenValue::HeapPtr {
                heap_id,
                value_index,
                is_str,
            } => self.resolve_heap_ptr(*heap_id, *value_index, *is_str),
            SerializedFrozenValue::InlineInt(v) => {
                let inline = InlineInt::try_from(*v)
                    .map_err(|_| anyhow::anyhow!("Integer {} does not fit in InlineInt", v))?;
                Ok(Value::new_int(inline))
            }
            SerializedFrozenValue::Static(id) => {
                let v = get_static_value_by_id(*id).ok_or_else(|| {
                    anyhow::anyhow!("Static value ID {:?} not found in inventory registry", id)
                })?;
                Ok(HeapEdge::immortal().rebrand(v))
            }
        }
    }

    /// Resolve a serialized HeapPtr into a value. Deserialize the target slot
    /// if needed; reads the header pointer from the slot's atomic.
    ///
    /// The pointers handed out here are the ones the framework wrote into the heap the serialized
    /// pointer names, which is a heap the brand reaches (see
    /// `StarlarkDeserializerImpl::recover_from_pagable`); the
    /// `'fv`-branded results are the framework handing out its own pointers.
    fn resolve_heap_ptr(
        &mut self,
        heap_id: HeapRefId,
        value_index: u32,
        is_str: bool,
    ) -> crate::Result<Value<'fv>> {
        let target_heap = match self.scope.get_heap(&heap_id) {
            Some(heap) => heap,
            None => resolve_missing_heap(self.scope, self.storage, self.origin, heap_id)?,
        };
        if let Some(origin) = self.origin {
            origin.retain_dependency(&target_heap);
        }
        if target_heap.deser_state().is_none() {
            // Page-in reused a native heap that remained resident after page-out.
            // Use the serialization state to resolve its value.
            let target_heap_ptr = target_heap
                .downgrade()
                .expect("a registered deserialization heap must have an allocation")
                .heap_ptr();
            #[cfg(fbcode_build)]
            let storage_context = self.storage.storage_context();
            #[cfg(not(fbcode_build))]
            let storage_context = self.storage_context;
            let Some(value) = storage_context.get::<StarlarkSerState>().and_then(|state| {
                state.lookup_registered_value(target_heap_ptr, value_index, is_str)
            }) else {
                return Err(PagableError::NativeHeapValueNotRegistered {
                    heap_id,
                    value_index,
                }
                .into());
            };
            return Ok(value);
        }

        // A skeleton bound from a ref list has not loaded its header, and its
        // values and its own dependencies come with it.
        target_heap.ensure_header_loaded(self.scope, self.storage)?;
        let ptr = resolve_in_loaded_heap(self.scope, self.storage, &target_heap, value_index)?;
        let ptr = NonNull::new(ptr).expect("a resolved slot has a header");
        let tag = if is_str {
            PointerTags::StrFrozen as usize
        } else {
            PointerTags::OtherFrozen as usize
        };
        // SAFETY: the allocation is retained by the context's brand. Use the
        // serialized string tag without borrowing a potentially unwritten
        // header: a cycle-breaking construction reference is not readable yet.
        Ok(unsafe { Value::new_frozen_ptr_with_str_tag(ptr.cast().map_addr(|addr| addr | tag)) })
    }
}

/// Construction guards must leave scope before the caller waits for readiness.
fn deserialize_claim(
    scope: &StarlarkDeserScope,
    storage: &PagableStorageHandle,
    target_heap: &FrozenHeapArc,
    slot_claim: SlotClaimGuard<'_>,
) -> crate::Result<SlotReadiness> {
    let target = slot_claim.recipe();
    let _claim = scope
        .wait_graph
        .claim(slot_claim.value, std::thread::current().id());
    let _active = ActiveClaim::enter(
        &scope.wait_graph,
        slot_claim.value,
        slot_claim.state.heap_id,
    );

    // The caller may be reading a different stream (e.g. a pagable Arc body).
    // A fresh deserializer also gives concurrent resolves independent cursors.
    let recipe = slot_claim.state.header()?.recipe.dupe();
    let result = {
        let mut de = recipe.open(storage);
        // SAFETY: metadata parsing computed this position from the target
        // heap's offset table, so it is valid in this recipe's bytes.
        unsafe { de.seek(target.abs_pos) };
        StarlarkDeserializerImpl::recover_from_pagable_in(&mut *de, target_heap, |nested_ctx| {
            (target.vtable.starlark_deserialize)(target.raw_ptr, nested_ctx)
        })
    };
    if let Err(e) = result {
        slot_claim.abort(&e);
        return Err(e);
    }
    // SAFETY: the recipe's deserializer successfully initialized the payload.
    unsafe { slot_claim.publish() }
}

/// Resolve a pointer for graph construction in a heap whose blob is already read.
///
/// Takes only shareable state so pointers can be resolved on any thread.
///
/// Returns a raw pointer rather than a `Value`: a `Value<'v>` needs a heap in
/// scope that keeps it alive, which the caller supplies. The pointee lives as
/// long as the target heap.
///
/// During nested construction this can return an unfinished cycle reference,
/// recording a readiness dependency on the current claim. Outside construction
/// it waits for transitive readiness before letting the pointer escape.
pub(crate) fn resolve_in_loaded_heap(
    scope: &StarlarkDeserScope,
    storage: &PagableStorageHandle,
    target_heap: &FrozenHeapArc,
    value_index: u32,
) -> crate::Result<*mut AValueHeader> {
    let target_heap_ptr = target_heap
        .downgrade()
        .expect("a registered deserialization heap must have an allocation")
        .heap_ptr();
    // A resident heap has no deserialization state; callers handle it first.
    let target_state = target_heap
        .deser_state()
        .expect("resolving in a loaded heap requires deserialization state");

    let value_count = target_state.value_count(storage)?;
    if value_index as usize >= value_count {
        return Err(anyhow::anyhow!(
            "value_index {value_index} out of range for heap {:?} (size {value_count})",
            target_state.heap_id
        )
        .into());
    }

    // Fast path: slot and its construction dependencies are already ready.
    if let Some(ptr) = target_state.ready_header_ptr(value_index as usize) {
        return Ok(ptr);
    }

    let wait_graph = scope.wait_graph();
    // Process-local heap identity prevents unrelated same-name heaps from
    // sharing active claim/wait edges in the storage-global graph.
    let in_progress_key = HeapValueId {
        heap_ptr: target_heap_ptr,
        value_index,
    };
    let resolved = match target_state.try_claim(in_progress_key, storage)? {
        ClaimResult::Claimed(slot_claim) => {
            deserialize_claim(scope, storage, target_heap, slot_claim)?
        }
        ClaimResult::InProgress(ptr) => {
            let my_thread = std::thread::current().id();
            // Slot is mid-deserialization (re-entrant or another thread).
            // `_wait` must outlive the block below so other threads' cycle
            // checks observe this wait.
            let (_wait, cycle) = wait_graph.begin_wait_and_check_cycle(my_thread, in_progress_key);
            if cycle {
                let parent = ActiveClaim::current(wait_graph).ok_or_else(|| {
                    anyhow::anyhow!(
                        "an owning deserialization result depends on its unfinished constructor while waiting for construction"
                    )
                })?;
                parent.depend_on(scope, in_progress_key, target_heap)?;
                return Ok(ptr);
            }
            // No cycle - safe to block until the claimer finishes.
            target_state.wait_for_slot(value_index as usize, storage, WaitFor::Constructed)?
        }
        ClaimResult::Resolved(result) => result,
        ClaimResult::Failed => {
            return Err(target_state
                .partial_deserialization_error(value_index)
                .into());
        }
    };

    let ptr = match resolved {
        SlotReadiness::Ready(ptr) => return Ok(ptr),
        SlotReadiness::Pending(ptr) => ptr,
    };
    if let Some(parent) = ActiveClaim::current(wait_graph) {
        parent.depend_on(scope, in_progress_key, target_heap)?;
        return Ok(ptr);
    }
    let (_wait, cycle) =
        wait_graph.begin_wait_and_check_cycle(std::thread::current().id(), in_progress_key);
    if cycle {
        return Err(anyhow::anyhow!(
            "an owning deserialization result depends on its unfinished constructor while waiting for transitive readiness"
        )
        .into());
    }
    match target_state.wait_for_slot(value_index as usize, storage, WaitFor::Ready)? {
        SlotReadiness::Ready(ptr) => Ok(ptr),
        SlotReadiness::Pending(_) => unreachable!("owning readers wait for readiness"),
    }
}

#[cfg(all(test, feature = "pagable"))]
mod tests {
    use std::cell::Cell;
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::sync::TryLockError;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::thread::ThreadId;
    use std::time::Duration;

    use dupe::Dupe;
    use pagable::PagableDeserialize;
    use pagable::PagableDeserializer;
    use pagable::PagableSerialize;
    use pagable::testing::TestingDeserializer;
    use pagable::testing::TestingSerializer;

    use super::ActiveClaim;
    use super::ClaimResult;
    use super::DeferredResolveContext;
    use super::DeserWaitKey;
    use super::HeapValueId;
    use super::MIN_HEAP_BINDING_PRUNE_INTERVAL;
    use super::StarlarkDeserScope;
    use super::StarlarkDeserWaitGraph;
    use super::StarlarkDeserializerImpl;
    use super::StarlarkHeapBindings;
    use super::with_deferred_values;
    use crate::pagable::error::PagableError;
    use crate::pagable::serialized_frozen_value::SerializedFrozenValue;
    use crate::values::FrozenHeapName;
    use crate::values::OwnedFrozen;
    use crate::values::OwnedFrozenHeap;
    use crate::values::Value;
    use crate::values::layout::heap::name::StarlarkTestHeapName;
    use crate::values::layout::heap::sealed::FrozenHeapPtr;
    use crate::values::layout::heap::sealed::HeapAllocationOrigin;

    #[test]
    fn test_active_claim_conflicting_heap_binding_is_an_error() {
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("active value");
        });
        let owner = heap.seal(FrozenHeapName::user("active_claim_binding"));
        let mut ser = TestingSerializer::new();
        owner.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        drop(owner);
        let mut de = TestingDeserializer::new(&bytes);
        let restored = OwnedFrozen::<()>::pagable_deserialize(&mut de).unwrap();
        let heap = restored.heap_arc();
        let heap_id = heap.deser_state().unwrap().heap_id;
        let active_value = HeapValueId {
            heap_ptr: heap.downgrade().unwrap().heap_ptr(),
            value_index: 0,
        };

        let bound = OwnedFrozenHeap::new();
        bound.with(|heap| {
            heap.alloc("conflicting value");
        });
        let bound = bound.seal(FrozenHeapName::user("conflicting_binding"));
        let bound_heap = bound.heap_arc();
        let bound_weak = bound_heap.downgrade().unwrap();
        let bound_heap_ptr = bound_weak.heap_ptr();
        let graph = Arc::new(StarlarkDeserWaitGraph::default());
        let scope = StarlarkDeserScope::new(Arc::default(), graph.dupe());
        scope.register_heap(heap_id, bound_weak).unwrap();
        let _active = ActiveClaim::enter(&graph, active_value, heap_id);
        let error = ActiveClaim::current(&graph)
            .unwrap()
            .depend_on(&scope, active_value, heap)
            .unwrap_err();
        let crate::ErrorKind::Other(inner) = error.kind() else {
            panic!("unexpected error: {error:#}");
        };
        let PagableError::ConflictingHeapBinding {
            heap_id: conflicting_id,
            bound_heap_ptr: actual_bound_ptr,
            bound_origin,
            conflicting_heap_ptr,
            conflicting_origin,
            ..
        } = inner
            .downcast_ref::<PagableError>()
            .expect("typed binding error")
        else {
            panic!("unexpected error: {error:#}");
        };
        assert_eq!(*conflicting_id, heap_id);
        assert_eq!(*actual_bound_ptr, bound_heap_ptr.addr());
        assert_eq!(*bound_origin, HeapAllocationOrigin::Native);
        assert_eq!(*conflicting_heap_ptr, active_value.heap_ptr.addr());
        assert_eq!(
            *conflicting_origin,
            Some(HeapAllocationOrigin::Deserialized)
        );
        assert!(scope.is_heap_bound(heap_id, bound_heap).unwrap());
        assert!(
            graph
                .readiness
                .lock()
                .unwrap()
                .pending_writers(active_value)
                .is_empty()
        );
    }

    fn registered_native_heap(scope: &StarlarkDeserScope) -> OwnedFrozen<Value<'static>> {
        let heap: OwnedFrozen<Value<'static>> =
            OwnedFrozen::build(StarlarkTestHeapName::frozen_heap_name(), |heap| {
                heap.alloc("native").to_value()
            });
        assert!(heap.heap_arc().deser_state().is_none());
        scope
            .register_heap(
                heap.heap_arc().heap_ref_id().unwrap(),
                heap.heap_arc().downgrade().unwrap(),
            )
            .unwrap();
        heap
    }

    #[test]
    fn test_heap_bindings_reclaim_expired_native_heaps() {
        let bindings = Arc::new(StarlarkHeapBindings::default());
        let scope = StarlarkDeserScope::new(bindings.dupe(), Default::default());
        let live = registered_native_heap(&scope);
        for _ in 0..4 * MIN_HEAP_BINDING_PRUNE_INTERVAL {
            drop(registered_native_heap(&scope));
        }
        assert!(
            bindings.bindings.len() <= MIN_HEAP_BINDING_PRUNE_INTERVAL + 1,
            "dead native bindings must not accumulate across repeated page-ins: {} entries",
            bindings.bindings.len()
        );
        assert_eq!(
            scope
                .get_heap(&live.heap_arc().heap_ref_id().unwrap())
                .as_ref(),
            Some(live.heap_arc()),
            "cleanup must preserve the live allocation"
        );
    }

    #[test]
    fn test_heap_bindings_prune_interval_scales_with_live_heaps() {
        let bindings = Arc::new(StarlarkHeapBindings::default());
        let scope = StarlarkDeserScope::new(bindings.dupe(), Default::default());
        let live: Vec<_> = (0..4 * MIN_HEAP_BINDING_PRUNE_INTERVAL)
            .map(|_| registered_native_heap(&scope))
            .collect();
        let interval = bindings.registrations_until_prune.load(Ordering::Relaxed);
        assert_eq!(interval, live.len());
        for heap in &live {
            let id = heap.heap_arc().heap_ref_id().unwrap();
            assert_eq!(scope.get_heap(&id).as_ref(), Some(heap.heap_arc()));
            scope
                .register_heap(id, heap.heap_arc().downgrade().unwrap())
                .unwrap();
        }
        assert_eq!(
            bindings.registrations_until_prune.load(Ordering::Relaxed),
            interval,
            "re-registering the same allocation must not charge another sweep"
        );

        drop(live);
        for _ in 0..interval {
            drop(registered_native_heap(&scope));
        }
        assert_eq!(bindings.bindings.len(), 1);
        assert!(
            bindings.bindings.capacity() < interval,
            "cleanup must reclaim excess capacity before shortening the next interval"
        );
        assert_eq!(
            bindings.registrations_until_prune.load(Ordering::Relaxed),
            MIN_HEAP_BINDING_PRUNE_INTERVAL
        );
    }

    #[test]
    fn test_heap_bindings_prune_with_concurrent_registrations() {
        let bindings = Arc::new(StarlarkHeapBindings::default());
        let scope = StarlarkDeserScope::new(bindings.dupe(), Default::default());
        let ready = Barrier::new(4);
        let live = std::thread::scope(|threads| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    threads.spawn(|| {
                        let live = registered_native_heap(&scope);
                        ready.wait();
                        for _ in 0..4 * MIN_HEAP_BINDING_PRUNE_INTERVAL {
                            drop(registered_native_heap(&scope));
                        }
                        live
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });

        // Finish the current interval after all concurrent drops have completed.
        let interval = bindings.registrations_until_prune.load(Ordering::Relaxed);
        assert!(interval > 0);
        for _ in 0..interval {
            drop(registered_native_heap(&scope));
        }
        assert_eq!(bindings.bindings.len(), live.len() + 1);
        for heap in &live {
            assert_eq!(
                scope
                    .get_heap(&heap.heap_arc().heap_ref_id().unwrap())
                    .as_ref(),
                Some(heap.heap_arc())
            );
        }
    }

    #[test]
    fn test_index_publication_holds_claims_until_complete() {
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("contents");
        });
        let owner = heap.seal(FrozenHeapName::user("test_index_publication_holds_claims"));
        let mut ser = TestingSerializer::new();
        owner.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        drop(owner);

        let mut de = TestingDeserializer::new(&bytes);
        let restored = OwnedFrozen::<()>::pagable_deserialize(&mut de).unwrap();
        let state = restored.heap_arc().deser_state().unwrap();
        let mut published = false;
        state.refresh_serialization_index(
            || false,
            |_| {
                assert!(
                    matches!(state.claims.try_lock(), Err(TryLockError::WouldBlock)),
                    "index publication must exclude new claims"
                );
                assert!(
                    state.serialization_index_is_dirty(),
                    "the index must stay dirty until publication completes"
                );
                published = true;
            },
        );
        assert!(published);
        assert!(!state.serialization_index_is_dirty());
        assert!(state.claims.try_lock().is_ok(), "claims must resume");
        state.refresh_serialization_index(
            || true,
            |_| panic!("a clean, registered index must not be republished"),
        );
    }

    #[test]
    fn test_claim_invalidates_a_clean_serialization_index() {
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("unclaimed value");
        });
        let owner = heap.seal(FrozenHeapName::user("claim_invalidates_index"));
        let mut ser = TestingSerializer::new();
        owner.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        drop(owner);
        let mut de = TestingDeserializer::new(&bytes);
        let restored = OwnedFrozen::<()>::pagable_deserialize(&mut de).unwrap();
        let heap = restored.heap_arc();
        let state = heap.deser_state().unwrap();
        state.refresh_serialization_index(|| false, |entries| assert!(entries.is_empty()));
        assert!(!state.serialization_index_is_dirty());

        let value = HeapValueId {
            heap_ptr: heap.downgrade().unwrap().heap_ptr(),
            value_index: 0,
        };
        let ClaimResult::Claimed(_claim) = state.try_claim(value, &de.storage()).unwrap() else {
            panic!("the cold slot must be unclaimed");
        };
        assert!(state.serialization_index_is_dirty());
        let mut published = false;
        state.refresh_serialization_index(
            || true,
            |entries| {
                assert_eq!(
                    entries
                        .iter()
                        .map(|chunk| chunk.payload_offsets.len())
                        .sum::<usize>(),
                    1
                );
                published = true;
            },
        );
        assert!(published, "a new claim invalidates even a registered index");
        assert!(!state.serialization_index_is_dirty());
    }

    #[test]
    fn test_concurrent_index_refreshes_publish_once() {
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("contents");
        });
        let owner = heap.seal(FrozenHeapName::user("concurrent_index_refreshes"));
        let mut ser = TestingSerializer::new();
        owner.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        drop(owner);
        let mut de = TestingDeserializer::new(&bytes);
        let restored = OwnedFrozen::<()>::pagable_deserialize(&mut de).unwrap();
        let state = restored.heap_arc().deser_state().unwrap();

        // A clean arena still needs an initial registration in a new context.
        state
            .serialization_index_dirty
            .store(false, Ordering::Release);
        let registered = AtomicBool::new(false);
        let publications = AtomicUsize::new(0);
        let checked = Barrier::new(2);
        std::thread::scope(|threads| {
            for _ in 0..2 {
                threads.spawn(|| {
                    let first_check = Cell::new(true);
                    state.refresh_serialization_index(
                        || {
                            let result = registered.load(Ordering::Acquire);
                            if first_check.replace(false) {
                                // Both requests observe the missing index before
                                // either can acquire the lock and publish it.
                                checked.wait();
                            }
                            result
                        },
                        |_| {
                            publications.fetch_add(1, Ordering::Relaxed);
                            registered.store(true, Ordering::Release);
                        },
                    );
                });
            }
        });
        assert!(registered.load(Ordering::Acquire));
        assert_eq!(publications.load(Ordering::Relaxed), 1);
    }

    fn value_id(n: usize) -> HeapValueId {
        HeapValueId {
            heap_ptr: FrozenHeapPtr::testing_new(0x1000 + n * 0x100),
            value_index: 0,
        }
    }

    // Only used as a key, so an exited thread's id serves.
    fn spawn_for_id() -> ThreadId {
        let handle = std::thread::spawn(|| {});
        let id = handle.thread().id();
        handle.join().unwrap();
        id
    }

    #[test]
    fn test_deferred_read_requires_ready_value_inside_constructor() {
        let heap = OwnedFrozenHeap::new();
        heap.with(|heap| {
            heap.alloc("unfinished field target");
        });
        let owner = heap.seal(FrozenHeapName::user("deferred_read_root_boundary"));
        let mut ser = TestingSerializer::new();
        owner.pagable_serialize(&mut ser).unwrap();
        let bytes = ser.finish();
        drop(owner);
        let mut de = TestingDeserializer::new(&bytes);
        let restored = OwnedFrozen::<()>::pagable_deserialize(&mut de).unwrap();
        let heap = restored.heap_arc();
        let state = heap.deser_state().unwrap();
        let scope = StarlarkDeserializerImpl::get_or_create_scope(&mut de);
        let storage = de.storage();
        let value = HeapValueId {
            heap_ptr: heap.downgrade().unwrap().heap_ptr(),
            value_index: 0,
        };
        let ClaimResult::Claimed(_slot) = state.try_claim(value, &storage).unwrap() else {
            panic!("the cold slot must be unclaimed");
        };
        let graph = scope.wait_graph();
        let _claim = graph.claim(value, std::thread::current().id());
        let _active = ActiveClaim::enter(graph, value, state.heap_id);
        let serialized = SerializedFrozenValue::HeapPtr {
            heap_id: state.heap_id,
            value_index: 0,
            is_str: true,
        };
        let result = with_deferred_values(
            &[serialized],
            DeferredResolveContext {
                scope: &scope,
                storage: &storage,
                #[cfg(not(fbcode_build))]
                storage_context: de.storage_context(),
                origin: heap,
            },
            |values| values.next().unwrap().map(|_| ()),
        );
        let error = result.expect_err("a deferred read cannot publish its unfinished constructor");
        assert!(
            format!("{error:#}").contains("depends on its unfinished constructor"),
            "{error:#}"
        );
        assert!(
            ActiveClaim::current(graph).is_some(),
            "the enclosing construction context is restored after the read"
        );
    }

    #[test]
    fn test_field_and_value_wait_cycles_clean_up() {
        let graph = Arc::new(StarlarkDeserWaitGraph::default());
        let a = spawn_for_id();
        let b = spawn_for_id();
        let field_key = DeserWaitKey::Field(1);
        let field = graph.claim(field_key, a);
        let slot = graph.claim(value_id(2), b);
        let (wait_a, cycle) = graph.begin_wait_and_check_cycle(a, value_id(2));
        assert!(!cycle);
        let (wait_b, cycle) = graph.begin_wait_and_check_cycle(b, field_key);
        assert!(cycle, "a mixed field/value wait cycle must be detected");
        drop((wait_a, wait_b));
        let (wait, cycle) = graph.begin_wait_and_check_cycle(a, field_key);
        assert!(cycle, "a direct field read cycle must be detected");
        drop((wait, field, slot));
        let (wait, cycle) = graph.begin_wait_and_check_cycle(a, field_key);
        assert!(!cycle, "an unclaimed field has no readiness dependencies");
        drop(wait);
        assert!(graph.claimers.is_empty());
        assert!(graph.lock_waiters().is_empty());
    }

    #[test]
    fn test_two_live_claimers_detect_cycle_and_clean_up() {
        let graph = Arc::new(StarlarkDeserWaitGraph::default());
        let claims_ready = Arc::new(Barrier::new(2));
        let waits_ready = Arc::new(Barrier::new(2));
        let (tx, rx) = std::sync::mpsc::channel();
        let threads: Vec<_> = (0..2)
            .map(|i| {
                let tx = tx.clone();
                let graph = graph.dupe();
                let claims_ready = claims_ready.dupe();
                let waits_ready = waits_ready.dupe();
                std::thread::spawn(move || {
                    let me = std::thread::current().id();
                    let claim = graph.claim(value_id(i), me);
                    claims_ready.wait();
                    let (wait, cycle) = graph.begin_wait_and_check_cycle(me, value_id(1 - i));
                    // Both edges stay live until both verdicts have been computed.
                    waits_ready.wait();
                    drop(wait);
                    drop(claim);
                    tx.send(cycle).expect("test receiver is alive");
                })
            })
            .collect();
        drop(tx);
        let cycles = (0..2)
            .map(|_| {
                usize::from(
                    rx.recv_timeout(Duration::from_secs(10))
                        .expect("claimers must finish"),
                )
            })
            .sum::<usize>();
        for thread in threads {
            thread.join().expect("claimer panicked");
        }
        assert_eq!(cycles, 1, "the second wait edge must close the cycle");
        assert!(graph.claimers.is_empty());
        assert!(graph.lock_waiters().is_empty());
    }

    /// `has_cycle` reads `claimers` while other threads add and remove entries.
    /// Drives the claim/wait protocol from one thread under that churn, so a
    /// regression in the lock split shows as a wrong verdict rather than as a
    /// rare hang in `test_cross_thread_cycle_does_not_deadlock`.
    #[test]
    fn test_cycle_verdicts_hold_under_concurrent_claim_churn() {
        let graph = Arc::new(StarlarkDeserWaitGraph::default());
        let thread_a = spawn_for_id();
        let thread_b = spawn_for_id();

        let stop = Arc::new(AtomicBool::new(false));
        let churners: Vec<_> = (0..2)
            .map(|i| {
                let graph = graph.dupe();
                let stop = stop.dupe();
                std::thread::spawn(move || {
                    let me = std::thread::current().id();
                    // Runs until the main loop is done. The bound only guards
                    // against a main loop that never finishes, and fails loudly
                    // so it cannot quietly end the churn early instead.
                    let mut n = 0usize;
                    while !stop.load(Ordering::Relaxed) {
                        assert!(
                            n < 10_000_000,
                            "churn ran out before the main loop finished"
                        );
                        // Ids disjoint from the test's own.
                        let guard = graph.claim(value_id(0x10000 + i * 0x1000 + (n % 64)), me);
                        drop(guard);
                        std::thread::yield_now();
                        n += 1;
                    }
                })
            })
            .collect();

        for round in 0..1000 {
            let value_a = value_id(round * 2);
            let value_b = value_id(round * 2 + 1);
            let claim_a = graph.claim(value_a, thread_a);
            let claim_b = graph.claim(value_b, thread_b);

            // B waits on A's value: chain ends at a non-waiting thread.
            let (wait_b, cycle) = graph.begin_wait_and_check_cycle(thread_b, value_a);
            assert!(
                !cycle,
                "B waiting on {value_a:?} must not be a cycle: its claimer A waits on nothing"
            );

            // A waiting on B's value closes the loop: A -> B -> A.
            let (wait_a, cycle) = graph.begin_wait_and_check_cycle(thread_a, value_b);
            assert!(
                cycle,
                "A -> {value_b:?} -> B -> {value_a:?} -> A must be a cycle"
            );

            drop(wait_a);
            drop(wait_b);
            // `wait_b` was dropped above: a thread holds one `waiters` entry,
            // and a second live guard would overwrite it.
            let (wait_b2, cycle) = graph.begin_wait_and_check_cycle(thread_b, value_a);
            assert!(!cycle, "the cycle must dissolve once A stops waiting");

            drop(wait_b2);
            drop(claim_b);
            // The value is unclaimed now, so the walk ends there.
            let (wait_b3, cycle) = graph.begin_wait_and_check_cycle(thread_b, value_b);
            assert!(!cycle, "an unclaimed value cannot be part of a cycle");
            drop(wait_b3);
            drop(claim_a);
        }

        stop.store(true, Ordering::Relaxed);
        for churner in churners {
            churner.join().unwrap();
        }
    }
}
