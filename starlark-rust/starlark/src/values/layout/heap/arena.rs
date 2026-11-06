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

//! A heap storing AValue traits. Each allocation begins with an
//! [`AValueHeapEntry`] followed by its payload. The entry is a live value
//! header (including the vtable), a forwarding record, or a reservation
//! containing the allocation size. Allocations are large enough to hold a
//! forwarding record, even for zero-sized payloads.
//!
//! A reservation remains while its payload is initialized, then is replaced by
//! the live value header. During GC or freezing, a live source entry may be
//! replaced by a forwarding record. Reservations may be allowed to be partially
//! initialized, but if and only if the initializing code can Drop the
//! already-initialized state when it fails. IE: the heap itself treats
//! reservations as uninit.

use std::collections::HashMap;
use std::mem;
use std::mem::MaybeUninit;
use std::num::NonZeroU32;
use std::ptr;
use std::ptr::NonNull;
use std::slice;
use std::sync::Arc;
use std::sync::MutexGuard;
use std::sync::OnceLock;

use allocative::Allocative;
use allocative::Visitor;
use bumpalo::Bump;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableDeserializer;
use pagable::PagableSerialize;
use pagable::PagableSerializer;
use starlark_map::small_map::SmallMap;

use crate::collections::StarlarkHashValue;
use crate::eval::runtime::profile::instant::ProfilerInstant;
use crate::pagable::starlark_deserialize_context::HeapDeserializationState;
use crate::values::Value;
use crate::values::layout::aligned_size::AlignedSize;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::avalues::str_::starlark_str;
use crate::values::layout::heap::allocator::api::AllocatedChunk;
use crate::values::layout::heap::allocator::api::ArenaAllocator;
use crate::values::layout::heap::allocator::api::ChunkAllocationDirection;
use crate::values::layout::heap::call_enter_exit::CallEnter;
use crate::values::layout::heap::call_enter_exit::CallExit;
use crate::values::layout::heap::call_enter_exit::NeedsDrop;
use crate::values::layout::heap::call_enter_exit::NoDrop;
use crate::values::layout::heap::profile::alloc_counts::AllocCounts;
use crate::values::layout::heap::profile::by_type::HeapSummary;
use crate::values::layout::heap::repr::AValueForward;
use crate::values::layout::heap::repr::AValueHeader;
use crate::values::layout::heap::repr::AValueHeaderPtr;
use crate::values::layout::heap::repr::AValueHeapEntry;
#[cfg(debug_assertions)]
use crate::values::layout::heap::repr::AValueHeapEntryState;
use crate::values::layout::heap::repr::AValueRepr;
use crate::values::layout::heap::repr::ForwardPtr;
use crate::values::layout::heap::repr::WalkState;
use crate::values::layout::value_alloc_size::ValueAllocSize;
use crate::values::layout::vtable::AValueVTable;
use crate::values::string::str_type::StarlarkStr;

/// Min size of allocated object including the entry.
/// Every allocation must fit any heap-entry record written in place over its
/// lifetime: a live value header, a reservation word, or a forward.
pub(crate) const MIN_ALLOC: AlignedSize = {
    const fn max(a: AlignedSize, b: AlignedSize) -> AlignedSize {
        if a.bytes() > b.bytes() { a } else { b }
    }

    max(
        AlignedSize::of::<AValueForward>(),
        AlignedSize::of::<AValueHeapEntry>(),
    )
};

/// Whether values are read out of an unfrozen or a frozen heap: the two publish their
/// pointers with different tags, see `Value`.
#[derive(Copy, Clone, Dupe)]
pub(crate) enum HeapKind {
    Unfrozen,
    Frozen,
}

/// Which bump region a value is allocated in.
#[derive(Allocative, Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) enum BumpKind {
    Drop = 0,
    NonDrop = 1,
}

impl PagableSerialize for BumpKind {
    fn pagable_serialize(&self, serializer: &mut dyn PagableSerializer) -> pagable::Result<()> {
        (*self as u8).pagable_serialize(serializer)
    }
}

impl<'de> PagableDeserialize<'de> for BumpKind {
    fn pagable_deserialize<D: PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        match u8::pagable_deserialize(deserializer)? {
            0 => Ok(BumpKind::Drop),
            1 => Ok(BumpKind::NonDrop),
            tag => Err(anyhow::anyhow!("Invalid BumpKind tag: {}", tag)),
        }
    }
}

/// Location of a value within a heap's arena.
#[derive(Copy, Clone, Debug, PagableSerialize, PagableDeserialize)]
pub(crate) struct ArenaOffset {
    pub(crate) bump: BumpKind,
    pub(crate) offset: u32,
}

/// A chunk's base pointer, for the serializer to build value pointers from, so that those carry
/// the chunk's provenance rather than being conjured from an address.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ChunkBase(NonNull<u8>);

// SAFETY: An address as far as its users are concerned: what it points at is reached only
// through the heap that owns the chunk.
unsafe impl Send for ChunkBase {}
unsafe impl Sync for ChunkBase {}

impl Allocative for ChunkBase {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        visitor.visit_simple_sized::<Self>();
    }
}

impl ChunkBase {
    fn new(ptr: *mut u8) -> ChunkBase {
        ChunkBase(NonNull::new(ptr).expect("an allocated chunk is not at null"))
    }

    pub(crate) fn addr(self) -> usize {
        self.0.addr().get()
    }

    /// The pointer `offset` bytes into the chunk.
    pub(crate) fn add(self, offset: usize) -> *const u8 {
        self.0.as_ptr().wrapping_add(offset).cast_const()
    }
}

/// Per-chunk record returned by [`Arena::build_chunk_index`]. One entry
/// per allocated chunk in serialization order (drop bump, then non-drop).
#[derive(Debug)]
pub(crate) struct ChunkInfo {
    /// Chunk's base.
    pub(crate) base: ChunkBase,
    /// Chunk size in bytes.
    pub(crate) size: u32,
    /// Cumulative count of values in earlier chunks (this heap's
    /// serialization order).
    pub(crate) values_before: u32,
    /// Sorted within-chunk byte offsets of each live value payload pointer.
    pub(crate) payload_offsets: Vec<u32>,
}

/// Cursor for writing values into a pre-allocated raw block in the arena.
/// Handles allocation direction internally so callers don't need to care.
pub(crate) struct ArenaRawCursor {
    cursor: *mut u8,
    direction: ChunkAllocationDirection,
}

impl ArenaRawCursor {
    /// Get pointer to the next value slot and advance cursor by `alloc_size`.
    /// Returns a pointer to the `AValueHeader` position for this value.
    pub(crate) unsafe fn next(&mut self, alloc_size: u32) -> *mut AValueHeader {
        unsafe {
            match self.direction {
                ChunkAllocationDirection::Up => {
                    let ptr = self.cursor;
                    self.cursor = self.cursor.add(alloc_size as usize);
                    ptr as *mut AValueHeader
                }
                ChunkAllocationDirection::Down => {
                    self.cursor = self.cursor.sub(alloc_size as usize);
                    self.cursor as *mut AValueHeader
                }
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct Arena<A: ArenaAllocator> {
    /// Arena for things which don't need dropping (e.g. strings)
    non_drop: A,
    /// Arena for things which might need dropping (e.g. Vec, with memory on heap)
    drop: A,
    /// Sizes claimed slots the sentinel vtable cannot size, and excludes new
    /// claims during walks. Set once for a restored heap; never for a heap
    /// built here.
    deserialization_state: OnceLock<Arc<HeapDeserializationState>>,
}

/// An entry met while walking an arena, decoded once per step
/// ([`AValueHeapEntry::walk_state`]) so walkers match on the kind. Unlike
/// value access, a walk can meet a claimed slot whose value is not there, so
/// that is a kind of its own rather than a value with a panicking vtable.
pub(crate) enum ArenaEntry<'a> {
    Value(AValueHeaderPtr<'a>),
    /// A claimed slot this walk cannot expose: its payload is unwritten, or
    /// its construction dependencies are pending or failed. Treat both the
    /// payload and header as opaque.
    Uninitialized {
        payload: *const (),
        size: ValueAllocSize,
    },
    Forward(&'a AValueForward),
    Reservation(ValueAllocSize),
}

#[derive(Clone, Copy)]
enum WalkMode {
    /// Expose only values whose construction dependencies are also ready.
    Readable,
    /// Inspect allocation addresses/sizes or drop written payloads, including
    /// those whose dependencies are pending or failed. Never follow value edges.
    Physical,
}

impl<'a> ArenaEntry<'a> {
    /// # Safety
    /// See [`AValueHeapEntry::walk_state`].
    unsafe fn classify(
        entry: *const AValueHeapEntry,
        sizes: Option<&HeapDeserializationState>,
        mode: WalkMode,
    ) -> ArenaEntry<'a> {
        // SAFETY: the caller guarantees the header's lifetime and synchronization.
        match unsafe { AValueHeapEntry::walk_state(entry) } {
            WalkState::Value(header) => {
                let payload = header.payload_ptr().ptr;
                if matches!(mode, WalkMode::Readable)
                    && let Some(size) = sizes.and_then(|sizes| sizes.unreadable_slot_size(payload))
                {
                    ArenaEntry::Uninitialized { payload, size }
                } else {
                    ArenaEntry::Value(header)
                }
            }
            WalkState::Uninitialized { payload } => {
                let size = sizes
                    .and_then(|sizes| sizes.uninitialized_slot_size(payload))
                    .expect("an uninitialized slot is one its heap's deserialization claimed");
                ArenaEntry::Uninitialized { payload, size }
            }
            WalkState::Forward(forward) => ArenaEntry::Forward(forward),
            WalkState::Reservation(size) => ArenaEntry::Reservation(size),
        }
    }

    /// Size of the allocation; the next entry follows it.
    fn size(&self) -> ValueAllocSize {
        match self {
            ArenaEntry::Value(header) => header.unpack().memory_size(),
            ArenaEntry::Uninitialized { size, .. } | ArenaEntry::Reservation(size) => *size,
            ArenaEntry::Forward(forward) => forward.object_size(),
        }
    }

    /// The header of an initialized payload, filtered by the walk's mode.
    fn value_header(&self) -> Option<AValueHeaderPtr<'a>> {
        match self {
            ArenaEntry::Value(header) => Some(*header),
            ArenaEntry::Uninitialized { .. }
            | ArenaEntry::Forward(_)
            | ArenaEntry::Reservation(_) => None,
        }
    }
}

/// Reservation is morally a Reservation<T>, but we treat is as an
/// existential.
/// Tied to the lifetime of the heap.
pub(crate) struct Reservation<'v, T: AValue<'v>> {
    pointer: *mut AValueRepr<T::StarlarkValue>,
}

impl<'v, T: AValue<'v>> Reservation<'v, T> {
    /// Target for the source's forwarding record. The destination holds no
    /// value until [`Reservation::fill`] publishes one.
    pub(crate) fn forward_ptr(&self) -> ForwardPtr {
        // SAFETY: the arena never hands out a null reservation.
        ForwardPtr::new(unsafe { NonNull::new_unchecked(self.pointer) }.cast())
    }

    /// Publishes the value and returns its now-valid header.
    pub(in crate::values::layout) fn fill(self, x: T::StarlarkValue) -> NonNull<AValueHeader> {
        // SAFETY: `pointer` owns an allocation sized and aligned for this representation,
        // and consuming the reservation prevents a second initialization.
        unsafe {
            #[cfg(debug_assertions)]
            let reserved = match (&*self.entry_ptr()).state() {
                AValueHeapEntryState::Reservation(size) => size,
                _ => unreachable!("a live reservation still holds its reservation word"),
            };
            ptr::write(
                self.pointer,
                AValueRepr {
                    header: AValueHeader::new::<T>(),
                    payload: x,
                },
            );
            let header = NonNull::new_unchecked(self.pointer.cast::<AValueHeader>());
            // Heap walks stride by the published header's payload-derived size,
            // so it must reproduce the reservation exactly.
            #[cfg(debug_assertions)]
            debug_assert_eq!(
                header.as_ref().alloc_size(),
                reserved,
                "published `{}` reports a different size than its reservation",
                std::any::type_name::<T::StarlarkValue>(),
            );
            header
        }
    }

    pub(in crate::values::layout) fn entry_ptr(&self) -> *mut AValueHeapEntry {
        self.pointer.cast()
    }

    pub(in crate::values::layout) fn into_entry_ptr(self) -> *mut AValueHeapEntry {
        self.pointer.cast()
    }
}

/// A [`Reservation`] on the frozen heap branded `'fh` for a value typed at that brand; filling
/// it publishes a frozen [`Value<'fh>`](Value).
pub(crate) struct FrozenReservation<'fh, T: AValue<'fh>>(
    pub(in crate::values::layout) Reservation<'fh, T>,
);

impl<'fh, T: AValue<'fh>> FrozenReservation<'fh, T> {
    pub(crate) fn forward_ptr(&self) -> ForwardPtr {
        self.0.forward_ptr()
    }

    pub(crate) fn into_entry_ptr(self) -> *mut AValueHeapEntry {
        self.0.into_entry_ptr()
    }

    pub(crate) fn fill(self, x: T::StarlarkValue) -> Value<'fh> {
        let header = self.0.fill(x);
        // SAFETY: `fill` published the value header on the frozen heap this
        // reservation was branded from, by the allocation's pointer, and
        // reservations are never strings.
        unsafe { Value::new_frozen_ptr(header, false) }
    }
}

/// A [`Reservation`] on a mutable heap; filling it publishes a [`Value`].
pub(crate) struct ValueReservation<'v, T: AValue<'v>>(
    pub(in crate::values::layout) Reservation<'v, T>,
);

impl<'v, T: AValue<'v>> ValueReservation<'v, T> {
    pub(crate) fn forward_ptr(&self) -> ForwardPtr {
        self.0.forward_ptr()
    }

    pub(crate) fn fill(self, x: T::StarlarkValue) -> Value<'v> {
        let header = self.0.fill(x);
        // SAFETY: `fill` published the value header, by the allocation's pointer, and
        // reservations are never strings.
        unsafe { Value::new_ptr(header, false) }
    }
}

pub(crate) trait ArenaVisitor<'v> {
    fn enter_bump(&mut self);
    fn regular_entry(&mut self, entry: ArenaEntry<'v>);
    fn call_enter(&mut self, function: Value<'v>, time: ProfilerInstant);
    fn call_exit(&mut self, time: ProfilerInstant);
}

/// Iterate over chunk contents.
struct ChunkIter<'c> {
    chunk: AllocatedChunk<'c>,
    sizes: Option<&'c HeapDeserializationState>,
    mode: WalkMode,
}

impl<'c> Iterator for ChunkIter<'c> {
    type Item = ArenaEntry<'c>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if self.chunk.is_empty() {
                None
            } else {
                // Claims were excluded while collecting the range, so every
                // entry has a header. Only sentinel-to-value publication may
                // overlap this walk; no reference may precede the acquire load.
                let kind = ArenaEntry::classify(self.chunk.as_ptr().cast(), self.sizes, self.mode);
                let n = kind.size();
                self.chunk.advance(n.bytes() as usize);
                Some(kind)
            }
        }
    }
}

/// Result of allocation. Both fields are uninitialized.
pub(crate) struct ArenaUninit<'v, T: AValue<'v>> {
    // We use `MaybeUninit` here to emphasize that the memory is uninitialized.
    // Invariant: `repr` points to an allocation of
    // `T::alloc_size_for_extra_len(extra.len())` bytes.
    repr: *mut MaybeUninit<AValueRepr<T::StarlarkValue>>,
    extra: *mut [MaybeUninit<T::ExtraElem>],
}

impl<'v, T: AValue<'v>> ArenaUninit<'v, T> {
    pub(crate) fn write_reservation(
        self,
    ) -> (Reservation<'v, T>, *mut [MaybeUninit<T::ExtraElem>]) {
        // SAFETY: By the type invariant, `repr` is an allocation of
        // `T::alloc_size_for_extra_len(self.extra.len())` bytes, so writing the first
        // word is in bounds and the reservation encodes the allocation's true size.
        unsafe {
            let p = self.repr as *mut AValueRepr<T::StarlarkValue>;
            p.cast::<AValueHeapEntry>()
                .write(AValueHeapEntry::new_reservation(
                    T::alloc_size_for_extra_len(self.extra.len()),
                ));
            (Reservation { pointer: p }, self.extra)
        }
    }

    pub(crate) fn debug_assert_extra_is_empty(&self) {
        let extra = unsafe { &*self.extra };
        debug_assert!(extra.is_empty());
    }

    pub(crate) fn write(
        self,
        x: T::StarlarkValue,
    ) -> (
        *mut AValueRepr<AValueImpl<'v, T>>,
        *mut [MaybeUninit<T::ExtraElem>],
    ) {
        unsafe {
            let repr = self.repr as *mut AValueRepr<AValueImpl<'v, T>>;
            repr.write(AValueRepr {
                header: AValueHeader::new::<T>(),
                payload: AValueImpl::new(x),
            });
            (repr, self.extra)
        }
    }

    pub(crate) fn write_no_extra(self, x: T::StarlarkValue) -> *mut AValueRepr<AValueImpl<'v, T>> {
        self.debug_assert_extra_is_empty();
        self.write(x).0
    }
}

enum ArenaVisitEvent<'a> {
    /// Called when entering new bump.
    EnterBump,
    /// Visiting a heap entry in the bump.
    Entry(ArenaEntry<'a>),
}

impl<A: ArenaAllocator> Arena<A> {
    pub(crate) fn is_empty(&self) -> bool {
        self.allocated_bytes() == 0
    }

    /// Number of allocated bytes plus padding size.
    pub(crate) fn allocated_bytes(&self) -> usize {
        // This overestimates the allocates size, see `Bump::allocated_bytes()`:
        // "those padding bytes will add to this method’s resulting sum".
        // https://docs.rs/bumpalo/3.11.0/bumpalo/struct.Bump.html#method.allocated_bytes
        self.drop.allocated_bytes() + self.non_drop.allocated_bytes()
    }

    pub(crate) fn available_bytes(&self) -> usize {
        self.drop.remaining_capacity() + self.non_drop.remaining_capacity()
    }

    /// Don't forget to call this function to release memory.
    pub(crate) fn finish(&mut self) {
        self.drop.finish();
        self.non_drop.finish();
    }

    fn alloc_uninit<'v, 'v2, T: AValue<'v2>>(bump: &'v A, extra_len: usize) -> ArenaUninit<'v2, T> {
        let () = AValueRepr::<T::StarlarkValue>::PAYLOAD_ALIGNED;

        let size = T::alloc_size_for_extra_len(extra_len);
        let p = bump.alloc(size).as_ptr();
        unsafe {
            let repr = p as *mut MaybeUninit<AValueRepr<_>>;
            let extra = slice::from_raw_parts_mut(
                p.add(AValueRepr::<T>::offset_of_extra()) as *mut _,
                extra_len,
            );
            ArenaUninit { repr, extra }
        }
    }

    fn bump_for_type<'v, T: AValue<'v>>(&self) -> &A {
        if T::NEEDS_DROP {
            &self.drop
        } else {
            &self.non_drop
        }
    }

    /// Reserves uninitialized storage for a value and its trailing elements.
    ///
    /// The reservation header keeps the allocation walkable until the final
    /// value header is published.
    pub(crate) fn reserve_with_extra<'v2, T: AValue<'v2>>(
        &self,
        extra_len: usize,
    ) -> (Reservation<'v2, T>, *mut [MaybeUninit<T::ExtraElem>]) {
        // Filling a reservation publishes its value with the string pointer
        // tag unset, so a string must never be allocated through one.
        assert!(!T::IS_STR);

        let arena_uninit = Self::alloc_uninit::<T>(self.bump_for_type::<T>(), extra_len);
        // Publish the allocation size before exposing uninitialized storage. If
        // initialization fails, arena walkers can advance without reading payload bytes.
        arena_uninit.write_reservation()
    }

    /// Allocate a type `T`.
    pub(crate) fn alloc<'v, 'v2, T: AValue<'v2, ExtraElem = ()>>(
        &'v self,
        x: AValueImpl<'v2, T>,
    ) -> NonNull<AValueRepr<AValueImpl<'v2, T>>> {
        debug_assert!(T::extra_len(&x.1) == 0);
        let bump = self.bump_for_type::<T>();
        let arena_uninit = Self::alloc_uninit::<T>(bump, 0);
        arena_uninit.debug_assert_extra_is_empty();
        // SAFETY: the arena never allocates at null.
        unsafe { NonNull::new_unchecked(arena_uninit.write_no_extra(x.1)) }
    }

    /// Allocate a type `T` plus `extra` bytes.
    pub(crate) fn alloc_extra<'v, T: AValue<'v>>(
        &self,
        x: AValueImpl<'v, T>,
    ) -> (
        *mut AValueRepr<AValueImpl<'v, T>>,
        *mut [MaybeUninit<T::ExtraElem>],
    ) {
        let bump = self.bump_for_type::<T>();
        let extra_len = T::extra_len(&x.1);
        let arena_uninit = Self::alloc_uninit::<T>(bump, extra_len);
        let (p, extra) = arena_uninit.write(x.1);
        (p, extra)
    }

    #[inline]
    pub(crate) fn alloc_str_init(
        &self,
        len: usize,
        hash: StarlarkHashValue,
        init: impl FnOnce(*mut u8),
    ) -> *mut AValueHeader {
        assert!(len > 1);
        let (v, extra) = self.alloc_extra::<_>(starlark_str(len, hash));
        let extra = unsafe { &mut *extra };
        debug_assert_eq!(StarlarkStr::payload_len_for_len(len), extra.len());
        unsafe {
            extra.last_mut().unwrap_unchecked().write(0usize);
        }
        init(extra.as_mut_ptr() as *mut u8);
        unsafe { &mut (*v).header }
    }

    #[inline]
    pub(crate) fn alloc_str(&self, x: &str) -> *mut AValueHeader {
        self.alloc_str_init(x.len(), StarlarkStr::UNINIT_HASH, |dest| unsafe {
            ptr::copy_nonoverlapping(x.as_ptr(), dest, x.len())
        })
    }

    /// Install the state shared with deserialization exactly once.
    pub(crate) fn set_deserialization_state(&self, state: Arc<HeapDeserializationState>) {
        assert!(self.deserialization_state.set(state).is_ok());
    }

    fn deserialization_state(&self) -> Option<&HeapDeserializationState> {
        self.deserialization_state.get().map(Arc::as_ref)
    }

    /// Exclude allocation and sentinel installation while inspecting chunk metadata.
    fn hold_claims(&self) -> Option<MutexGuard<'_, ()>> {
        self.deserialization_state()
            .map(HeapDeserializationState::hold_claims)
    }

    fn iter_chunk<'a>(
        chunk: AllocatedChunk<'a>,
        sizes: Option<&'a HeapDeserializationState>,
        mode: WalkMode,
    ) -> ChunkIter<'a> {
        ChunkIter { chunk, sizes, mode }
    }

    /// Iterate over values in a single bump allocator in allocation order.
    fn for_each_bump_ordered<'a>(&'a self, bump: &'a A, mut f: impl FnMut(ArenaEntry<'a>)) {
        // We get the chunks from newest to oldest as per the bumpalo spec.
        // And within each chunk, the values are filled newest to oldest.
        // So need to do two sets of reversing.
        let chunks = {
            let _claims_held = self.hold_claims();
            // SAFETY: claims cannot change allocator metadata during collection.
            unsafe { bump.iter_allocated_chunks_rev().collect::<Vec<_>>() }
        };
        let sizes = self.deserialization_state();
        let mut buffer = Vec::new();
        for chunk in chunks.into_iter().rev() {
            match A::CHUNK_ALLOCATION_DIRECTION {
                ChunkAllocationDirection::Down => {
                    buffer.extend(Arena::<A>::iter_chunk(chunk, sizes, WalkMode::Readable));
                    for kind in buffer.drain(..).rev() {
                        f(kind);
                    }
                }
                ChunkAllocationDirection::Up => {
                    for kind in Arena::<A>::iter_chunk(chunk, sizes, WalkMode::Readable) {
                        f(kind);
                    }
                }
            }
        }
    }

    // Iterate over the values in the heap in the order they
    // were added.
    fn for_each_ordered<'a>(&'a self, mut f: impl FnMut(ArenaVisitEvent<'a>)) {
        for bump in [&self.drop, &self.non_drop] {
            f(ArenaVisitEvent::EnterBump);
            self.for_each_bump_ordered(bump, |kind| f(ArenaVisitEvent::Entry(kind)));
        }
    }

    /// Collect live value headers from the `drop` bump in allocation order.
    /// Forward pointers (from GC) and uninitialized slots are skipped.
    pub(crate) fn collect_drop_headers_ordered(&self) -> Vec<&AValueHeader> {
        self.collect_bump_headers_ordered(&self.drop)
    }

    /// Collect live value headers from the `non_drop` bump in allocation order.
    /// Forward pointers (from GC) and uninitialized slots are skipped.
    pub(crate) fn collect_undrop_headers_ordered(&self) -> Vec<&AValueHeader> {
        self.collect_bump_headers_ordered(&self.non_drop)
    }

    /// Allocate space for a single value of `alloc_size` bytes in the given
    /// bump and return a pointer to its `AValueHeader`. Used by partial-deser
    /// to allocate values one at a time.
    pub(crate) fn alloc_raw_one(
        &self,
        bump_kind: BumpKind,
        alloc_size: NonZeroU32,
    ) -> *mut AValueHeader {
        let bump = match bump_kind {
            BumpKind::Drop => &self.drop,
            BumpKind::NonDrop => &self.non_drop,
        };
        let mut cursor = Self::alloc_raw_cursor(bump, alloc_size);
        // SAFETY: cursor has exactly alloc_size bytes available.
        unsafe { cursor.next(alloc_size.get()) }
    }

    fn alloc_raw_cursor(bump: &A, total_bytes: NonZeroU32) -> ArenaRawCursor {
        let total_bytes = total_bytes.get();
        let size = ValueAllocSize::new(AlignedSize::new_bytes(total_bytes as usize));
        let block = bump.alloc(size);
        let cursor = match A::CHUNK_ALLOCATION_DIRECTION {
            ChunkAllocationDirection::Up => block.as_ptr(),
            ChunkAllocationDirection::Down => unsafe { block.as_ptr().add(total_bytes as usize) },
        };
        ArenaRawCursor {
            cursor,
            direction: A::CHUNK_ALLOCATION_DIRECTION,
        }
    }

    fn collect_bump_headers_ordered<'a>(&'a self, bump: &'a A) -> Vec<&'a AValueHeader> {
        let mut headers = Vec::new();
        self.for_each_bump_ordered(bump, |kind| {
            if let Some(header) = kind.value_header() {
                headers.push(header.header());
            }
        });
        headers
    }

    pub(crate) fn allocated_chunk_bases(&self) -> Vec<usize> {
        fn append<A: ArenaAllocator>(bump: &A, bases: &mut Vec<usize>) {
            bases.extend(
                unsafe { bump.iter_allocated_chunks_rev() }.map(|chunk| chunk.as_ptr() as usize),
            );
        }

        let mut bases = Vec::new();
        append(&self.drop, &mut bases);
        append(&self.non_drop, &mut bases);
        bases
    }

    /// Per-chunk index for serialization-time `ptr → value_index` lookup.
    /// Entries are in serialization order (drop bump first, then non-drop).
    pub(crate) fn build_chunk_index(&self) -> Vec<ChunkInfo> {
        let _claims_held = self.hold_claims();
        self.build_chunk_index_claims_held()
    }

    /// Check, rebuild if needed, and publish under the same claim hold.
    /// Concurrent refreshes coalesce and cannot publish a stale index.
    pub(crate) fn refresh_chunk_index(
        &self,
        is_current: impl FnOnce() -> bool,
        publish: impl FnOnce(Vec<ChunkInfo>),
    ) {
        let _claims_held = self.hold_claims();
        if !is_current() {
            publish(self.build_chunk_index_claims_held());
        }
    }

    fn build_chunk_index_claims_held(&self) -> Vec<ChunkInfo> {
        let mut entries = Vec::new();
        let mut values_before: u32 = 0;

        fn build_for_bump<A: ArenaAllocator>(
            bump: &A,
            sizes: Option<&HeapDeserializationState>,
            entries: &mut Vec<ChunkInfo>,
            values_before: &mut u32,
        ) {
            // `iter_allocated_chunks_rev` yields newest-first; reverse for
            // allocation order so `values_before` is monotonic.
            let chunks_rev: Vec<_> = unsafe { bump.iter_allocated_chunks_rev() }.collect();
            for chunk in chunks_rev.into_iter().rev() {
                let base_ptr = ChunkBase::new(chunk.as_ptr().cast::<u8>() as *mut u8);
                let base = base_ptr.addr();
                let size = chunk.len() as u32;
                // A claimed slot is a value's address whether or not the value arrived.
                let mut payload_offsets: Vec<u32> =
                    Arena::<A>::iter_chunk(chunk, sizes, WalkMode::Physical)
                        .filter_map(|kind| match kind {
                            ArenaEntry::Value(header) => Some(header.payload_ptr().ptr),
                            ArenaEntry::Uninitialized { payload, .. } => Some(payload),
                            ArenaEntry::Forward(_) | ArenaEntry::Reservation(_) => None,
                        })
                        .map(|payload| (payload as usize - base) as u32)
                        .collect();
                // Sort for binary_search at lookup time. For `Up` allocators
                // this is a no-op (already ascending); kept for safety across
                // allocator directions.
                payload_offsets.sort_unstable();
                let count = payload_offsets.len() as u32;
                entries.push(ChunkInfo {
                    base: base_ptr,
                    size,
                    values_before: *values_before,
                    payload_offsets,
                });
                *values_before += count;
            }
        }

        let sizes = self.deserialization_state();
        build_for_bump(&self.drop, sizes, &mut entries, &mut values_before);
        build_for_bump(&self.non_drop, sizes, &mut entries, &mut values_before);

        entries
    }

    pub(crate) unsafe fn visit_arena<'v>(
        &'v self,
        heap_kind: HeapKind,
        forward_heap_kind: HeapKind,
        visitor: &mut impl ArenaVisitor<'v>,
    ) {
        unsafe {
            fn fix_function<'v>(function: Value<'v>, forward_heap_kind: HeapKind) -> Value<'v> {
                if function.is_frozen() {
                    return function;
                }

                unsafe {
                    match function
                        .0
                        .unpack_ptr()
                        .expect("int cannot be stored in heap")
                        .forward()
                    {
                        None => function,
                        Some(forward) => forward.forward_ptr().unpack_value(forward_heap_kind),
                    }
                }
            }

            self.for_each_ordered(|x| match x {
                ArenaVisitEvent::EnterBump => visitor.enter_bump(),
                ArenaVisitEvent::Entry(kind) => match kind {
                    ArenaEntry::Value(header) => {
                        let value = header.unpack_value(heap_kind);
                        if let Some(call_enter) = value.downcast_ref::<CallEnter<NeedsDrop>>() {
                            visitor.call_enter(
                                fix_function(call_enter.function, forward_heap_kind),
                                call_enter.time,
                            );
                        } else if let Some(call_enter) = value.downcast_ref::<CallEnter<NoDrop>>() {
                            visitor.call_enter(
                                fix_function(call_enter.function, forward_heap_kind),
                                call_enter.time,
                            );
                        } else if let Some(call_exit) = value.downcast_ref::<CallExit<NeedsDrop>>()
                        {
                            visitor.call_exit(call_exit.time);
                        } else if let Some(call_exit) = value.downcast_ref::<CallExit<NoDrop>>() {
                            visitor.call_exit(call_exit.time);
                        } else {
                            visitor.regular_entry(kind);
                        }
                    }
                    ArenaEntry::Uninitialized { .. }
                    | ArenaEntry::Forward(_)
                    | ArenaEntry::Reservation(_) => visitor.regular_entry(kind),
                },
            });
        }
    }

    /// Iterate over the live values in the drop bump in any order; a claimed
    /// slot with no value is skipped.
    pub(crate) fn for_each_drop_unordered<'a>(
        &'a mut self,
        mut f: impl FnMut(AValueHeaderPtr<'a>),
    ) {
        let sizes = self.deserialization_state.get().map(Arc::as_ref);
        let _claims_held = sizes.map(HeapDeserializationState::hold_claims);
        // SAFETY: the claim hold excludes restored-heap allocation throughout
        // this walk; exclusive access excludes native-heap allocation.
        unsafe {
            for chunk in self.drop.iter_allocated_chunks_rev() {
                for kind in Self::iter_chunk(chunk, sizes, WalkMode::Physical) {
                    if let Some(header) = kind.value_header() {
                        f(header);
                    }
                }
            }
        }
    }

    fn for_each_in_chunks<'a>(
        chunks: impl IntoIterator<Item = AllocatedChunk<'a>>,
        sizes: Option<&'a HeapDeserializationState>,
        mut f: impl FnMut(ArenaEntry<'a>),
    ) {
        for chunk in chunks {
            for kind in Self::iter_chunk(chunk, sizes, WalkMode::Readable) {
                f(kind);
            }
        }
    }

    // Iterate over the entries in both bumps in any order
    fn for_each_unordered<'a>(&'a self, mut f: impl FnMut(ArenaEntry<'a>)) {
        let sizes = self.deserialization_state();
        for bump in [&self.drop, &self.non_drop] {
            if sizes.is_some() {
                let chunks = {
                    let _claims_held = self.hold_claims();
                    // SAFETY: allocator metadata is protected by the claim hold.
                    unsafe { bump.iter_allocated_chunks_rev().collect::<Vec<_>>() }
                };
                Self::for_each_in_chunks(chunks, sizes, &mut f);
            } else {
                // SAFETY: native heaps do not allocate during a walk.
                Self::for_each_in_chunks(
                    unsafe { bump.iter_allocated_chunks_rev() },
                    sizes,
                    &mut f,
                );
            }
        }
    }

    // For each Rust-level type (the String) report how many entries there are in the heap, and how much size they consume
    pub(crate) fn allocated_summary(&self) -> HeapSummary {
        // Record how many times each header occurs
        // We deliberately hash by the AValueHeader for higher performance, less type lookup
        let mut entries: HashMap<AValueHeader, (&'static str, AllocCounts)> = HashMap::new();
        let f = |kind: ArenaEntry| match kind {
            ArenaEntry::Value(x) => {
                let v = x.unpack();
                let e = entries
                    .entry(x.dupe())
                    .or_insert_with(|| (v.vtable().type_name, AllocCounts::default()));
                e.1.count += 1;
                e.1.bytes += v.total_memory_for_profile()
            }
            ArenaEntry::Uninitialized { size, .. } => {
                let sentinel = AValueVTable::uninitialized_sentinel();
                let e = entries
                    .entry(AValueHeader(sentinel))
                    .or_insert_with(|| (sentinel.type_name, AllocCounts::default()));
                e.1.count += 1;
                e.1.bytes += size.bytes() as usize;
            }
            ArenaEntry::Forward(_) | ArenaEntry::Reservation(_) => {}
        };
        self.for_each_unordered(f);

        // For a given type, the AValueHeader isn't always unique
        // (if they get compiled in different translation units),
        // so not just a simple map.
        let mut summary = SmallMap::new();
        for (_, (name, counts)) in entries {
            *summary.entry(name).or_insert_with(AllocCounts::default) += counts;
        }
        HeapSummary { summary }
    }
}

impl<A: ArenaAllocator> Drop for Arena<A> {
    fn drop(&mut self) {
        self.for_each_drop_unordered(|x| {
            // Safe to convert to *mut because we are the only owner
            let value = x.payload_ptr();
            x.0.drop_in_place(value);
        });
    }
}

impl<A: ArenaAllocator> Allocative for Arena<A> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let Arena {
            drop,
            non_drop,
            deserialization_state: _,
        } = self;
        fn visit_bump<'a, 'b: 'a, A: ArenaAllocator>(
            arena: &Arena<A>,
            bump: &A,
            visitor: &'a mut Visitor<'b>,
        ) {
            let (chunks, allocated_bytes, allocation_overhead) = {
                let _claims_held = arena.hold_claims();
                // SAFETY: allocator metadata is protected by the claim hold.
                let chunks = unsafe { bump.iter_allocated_chunks_rev().collect::<Vec<_>>() };
                (chunks, bump.allocated_bytes(), bump.allocation_overhead())
            };
            let mut visitor =
                visitor.enter_unique(allocative::Key::new("data"), mem::size_of::<*const ()>());
            let mut allocated_visitor =
                visitor.enter(allocative::Key::new("allocated"), allocated_bytes);
            Arena::<A>::for_each_in_chunks(
                chunks,
                arena.deserialization_state(),
                |kind| match kind {
                    ArenaEntry::Value(x) => {
                        let key = x.0.type_as_allocative_key.clone();
                        let size = x.unpack().memory_size();
                        let mut object_visitor =
                            allocated_visitor.enter(key, size.bytes() as usize);
                        // We visit both drop and non-drop bumps, because although
                        // non-drop `Bump` cannot contain malloc pointers, it can still provide
                        // useful information about headers/payload/padding.
                        let value = x.unpack();
                        value.as_allocative().visit(&mut object_visitor);
                        value.visit_extra_allocative(&mut object_visitor);
                        object_visitor.exit();
                    }
                    // The bytes are held; what they would hold is not there to visit.
                    ArenaEntry::Uninitialized { size, .. } => allocated_visitor.visit_simple(
                        AValueVTable::uninitialized_sentinel()
                            .type_as_allocative_key
                            .clone(),
                        size.bytes() as usize,
                    ),
                    ArenaEntry::Forward(_) | ArenaEntry::Reservation(_) => {}
                },
            );
            allocated_visitor.exit();
            visitor.visit_simple(
                allocative::Key::new("allocation_overhead"),
                allocation_overhead,
            );
            visitor.exit();
        }

        let mut visitor = visitor.enter_self_sized::<Self>();
        {
            let mut visitor = visitor.enter(allocative::Key::new("drop"), mem::size_of::<Bump>());
            visit_bump(self, drop, &mut visitor);
            visitor.exit();
        }
        {
            let mut visitor =
                visitor.enter(allocative::Key::new("non_drop"), mem::size_of::<Bump>());
            visit_bump(self, non_drop, &mut visitor);
            visitor.exit();
        }
        visitor.exit();
    }
}

#[cfg(test)]
mod tests {
    use std::ptr::NonNull;

    use super::*;
    use crate::values::Value;
    use crate::values::any::StarlarkAny;
    use crate::values::layout::avalues::simple::AValueFrozen;

    fn to_repr(x: &AValueHeader) -> String {
        let mut s = String::new();
        // SAFETY: the test allocates frozen values, and `x` is the header of one.
        let value = unsafe { Value::new_frozen_ptr(NonNull::from(x), false) };
        value.get_ref().collect_repr(&mut s);
        s
    }

    fn mk_str(x: &str) -> AValueImpl<'static, impl AValue<'static, ExtraElem = ()> + use<>> {
        AValueImpl::<AValueFrozen<StarlarkAny<String>>>::new(StarlarkAny::new(x.to_owned()))
    }

    fn reserve_str<'v, T: AValue<'static>>(
        arena: &'v Arena<Bump>,
        _: &AValueImpl<'static, T>,
    ) -> Reservation<'static, T> {
        arena.reserve_with_extra::<T>(0).0
    }

    #[test]
    fn test_trait_arena_iteration() {
        // We want iteration to proceed in the same order as allocation,
        // otherwise profiling won't work
        const LIMIT: usize = 10000;
        let mut arena = Arena::default();
        let mut reserved = Vec::new();
        for i in 0..LIMIT {
            if i % 100 == 0 {
                let r = reserve_str(&arena, &mk_str(""));
                reserved.push((r, i));
            } else {
                arena.alloc(mk_str(&i.to_string()));
            }
        }
        assert!(!reserved.is_empty());
        for (r, i) in reserved {
            r.fill(mk_str(&i.to_string()).1);
        }

        // Not a functional part of the test, just makes sure we go through
        // the interesting cases (last time 56 was sufficient, so 10K is plenty of margin of error)
        assert!(
            arena.drop.iter_allocated_chunks().count() > 1,
            "Didn't allocate enough to test properly"
        );
        let mut j = 0;
        arena.for_each_ordered(|i| match i {
            ArenaVisitEvent::EnterBump => {}
            ArenaVisitEvent::Entry(kind) => {
                if let Some(i) = kind.value_header() {
                    assert_eq!(to_repr(&i), format!("{:?}", j.to_string()));
                    j += 1;
                }
            }
        });
        assert_eq!(j, LIMIT);
        j = 0;
        arena.for_each_drop_unordered(|_| j += 1);
        assert_eq!(j, LIMIT);
    }

    #[test]
    // Make sure that even if there are some reservations when we drop, we can still walk to heap
    fn drop_with_reservation() {
        let arena = Arena::default();
        arena.alloc(mk_str("test"));
        {
            let reservation = reserve_str(&arena, &mk_str(""));
            // SAFETY: The reserved payload is allocated for this type but remains
            // uninitialized, and arena traversal will neither read nor drop it.
            unsafe {
                ptr::write_bytes(ptr::addr_of_mut!((*reservation.pointer).payload), 0xab, 1);
            }
        }
        arena.alloc(mk_str("hello"));
        let mut res = Vec::new();
        let mut reservations = 0;
        arena.for_each_ordered(|x| match x {
            ArenaVisitEvent::EnterBump => {}
            ArenaVisitEvent::Entry(kind) => match kind {
                ArenaEntry::Value(x) => res.push(x),
                ArenaEntry::Uninitialized { .. } => panic!("unexpected uninitialized slot"),
                ArenaEntry::Forward(_) => panic!("unexpected forward"),
                ArenaEntry::Reservation(_) => reservations += 1,
            },
        });
        assert_eq!(reservations, 1);
        assert_eq!(res.len(), 2);
        assert_eq!(to_repr(&res[0]), "\"test\"");
        assert_eq!(to_repr(&res[1]), "\"hello\"");
    }

    #[test]
    fn test_allocated_summary() {
        let arena = Arena::<Bump>::default();
        arena.alloc(mk_str("test"));
        arena.alloc(mk_str("test"));
        let res = arena.allocated_summary().summary;
        assert_eq!(res.len(), 1);
        let entry = res.values().next().unwrap();
        assert_eq!(entry.count, 2);
        // Because `arena.allocated_bytes` over-approximates.
        assert!(entry.bytes <= arena.allocated_bytes());
    }

    #[test]
    fn test_is_empty() {
        let arena = Arena::<Bump>::default();
        assert!(arena.is_empty());
        arena.alloc_str("xyz");
        assert!(!arena.is_empty());
    }
}
