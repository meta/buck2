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

//! A frozen heap under construction ([`OwnedFrozenHeap`]) and the handle that allocates on it
//! ([`FrozenHeap`]).

use std::cell::RefCell;
use std::cell::RefMut;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Formatter;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::TryLockError;

use dupe::Dupe;
use dupe::IterDupedExt;
use starlark_map::small_set::SmallSet;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::collections::StarlarkHashValue;
use crate::values::HeapSendable;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenRef;
use crate::values::StringValue;
use crate::values::ValueTyped;
use crate::values::layout::avalue::AValue;
use crate::values::layout::avalue::AValueImpl;
use crate::values::layout::heap::allocator::alloc::allocator::ChunkAllocator;
use crate::values::layout::heap::arena::Arena;
use crate::values::layout::heap::arena::FrozenReservation;
use crate::values::layout::heap::name::FrozenHeapName;
use crate::values::layout::heap::profile::by_type::HeapSummary;
use crate::values::layout::heap::sealed::FrozenHeapArc;
use crate::values::layout::heap::send::HeapSyncable;
use crate::values::layout::value::Value;
use crate::values::string::intern::interner::StringValueInterner;

/// A frozen heap under construction: an owned heap on which values can be allocated through the
/// [`FrozenHeap`] handle that [`with`](OwnedFrozenHeap::with) hands out, until it is
/// [`seal`](OwnedFrozenHeap::seal)ed into an [`OwnedFrozen<()>`] that can be shared between
/// threads.
#[derive(Default)]
pub struct OwnedFrozenHeap {
    /// My memory.
    arena: Arena<ChunkAllocator>,
    /// Memory I depend on.
    refs: HeapReferences,
    /// String interner. Its entries are allocated in this heap and stored with the brand erased;
    /// `FrozenHeap::alloc_str_hashed` restores it.
    str_interner: RefCell<StringValueInterner<'static>>,
}

impl Debug for OwnedFrozenHeap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut x = f.debug_struct("OwnedFrozenHeap");
        x.field("bytes", &self.arena.allocated_bytes());
        x.field("refs", &self.refs.try_len());
        x.finish()
    }
}

impl OwnedFrozenHeap {
    /// Create a new, empty heap.
    pub fn new() -> Self {
        Self::default()
    }

    /// A heap that depends on the same heaps as the heap `refs` came from, now and as either of
    /// them gains dependencies, see `ModuleHeaps`.
    pub(in crate::values::layout::heap) fn sharing_references(refs: HeapReferences) -> Self {
        Self {
            refs,
            ..Self::default()
        }
    }

    /// Allocate on this heap through a [`FrozenHeap`] handle that is valid within `f`.
    ///
    /// Values allocated through the handle are branded with its lifetime, so they cannot outlive
    /// the call. To keep them, [`seal`](OwnedFrozenHeap::seal) the heap afterwards and pair them
    /// with the result; [`OwnedFrozen::build`] does both steps for a single root value.
    pub fn with<R>(&self, f: impl for<'fh> FnOnce(FrozenHeap<'fh>) -> R) -> R {
        f(FrozenHeap(self, PhantomData))
    }

    /// After all values have been allocated, seal the heap into a named [`OwnedFrozen<()>`], which
    /// can be [`clone`](Clone::clone)d, shared between threads, and keeps the values allocated on
    /// the heap alive.
    ///
    /// The `name` identifies this heap and should be unique across heaps.
    /// See [`OwnedFrozen::name`] for more details.
    pub fn seal(self, name: FrozenHeapName) -> OwnedFrozen<()> {
        self.seal_impl(Some(name), None)
    }

    /// Allocate a root value through the handle, then seal the heap and return the value kept
    /// alive by it.
    ///
    /// `f` can only produce the value at the handle's brand, so the value is paired with its
    /// owner by construction; [`OwnedFrozen::build`] is this on a fresh heap. The heap is sealed
    /// whether `f` succeeds or fails; if `f` panics it is dropped unsealed instead, which is fine
    /// here because nothing outside `f` can hold a value at its brand (a module's builder is
    /// different, see `ModuleHeaps`).
    pub fn seal_with<T, E, F>(self, name: FrozenHeapName, f: F) -> Result<OwnedFrozen<T>, E>
    where
        T: IsStaticType,
        for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
        for<'fh> F: FnOnce(FrozenHeap<'fh>) -> Result<T::Reinfect<'fh>, E>,
    {
        // The brand of the value is the borrow of `self`, which has to end before `self` can be
        // sealed, so the brand is erased before sealing rather than by `unchecked_new` afterwards.
        //
        // SAFETY: `'fh` is the brand of `self`, which is sealed right below into the owner the
        // value is paired with. Being closure-introduced, `'fh` names nothing else.
        let v = self.with(|fh| f(fh).map(|v| unsafe { OwnedFrozen::<T>::erase_brand(v) }));
        let sealed = self.seal_impl(Some(name), None);
        // SAFETY: `sealed` is the heap that `'fh` named.
        v.map(|v| unsafe { OwnedFrozen::from_erased(sealed, v) })
    }

    pub(crate) fn seal_impl(
        self,
        name: Option<FrozenHeapName>,
        peak_allocated_bytes: Option<usize>,
    ) -> OwnedFrozen<()> {
        let OwnedFrozenHeap {
            mut arena, refs, ..
        } = self;
        arena.finish();
        // A snapshot: a module's value heap goes on adding to a set it shares with this heap.
        let refs = refs.to_vec();
        if arena.is_empty() && refs.is_empty() {
            OwnedFrozen::default()
        } else {
            OwnedFrozen::for_heap(FrozenHeapArc::new_sealed(
                arena,
                refs.into_boxed_slice(),
                name,
                peak_allocated_bytes,
            ))
        }
    }

    /// Number of bytes allocated on this heap, not including any memory
    /// allocated outside of the starlark heap.
    pub fn allocated_bytes(&self) -> usize {
        self.arena.allocated_bytes()
    }

    /// Whether anything has been allocated on this heap.
    pub(crate) fn has_allocations(&self) -> bool {
        !self.arena.is_empty()
    }
}

/// A handle to an [`OwnedFrozenHeap`] on which values can be allocated. The values will be
/// annotated with the heap lifetime, see the `branding` module for what that means.
///
/// Obtained from [`OwnedFrozenHeap::with`], [`FrozenHeap::temp`] or [`OwnedFrozen::build`].
#[derive(Copy, Clone, Dupe)]
// `PhantomData` is needed to make the type invariant in `'fh` - without that, branding doesn't mean
// anything.
pub struct FrozenHeap<'fh>(&'fh OwnedFrozenHeap, PhantomData<fn(&'fh ()) -> &'fh ()>);

// The handle is a borrow of the heap, not a value kept alive by it: rebranding it through a
// `HeapEdge` would let the borrow outlive the scope that introduced it.
static_assertions::assert_not_impl_any!(FrozenHeap<'static>: ProvidesStaticType<'static>);

impl<'fh> Debug for FrozenHeap<'fh> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(self.0, f)
    }
}

impl<'fh> FrozenHeap<'fh> {
    /// Create a heap and use it within the closure.
    ///
    /// Heap is discarded at the end of the closure.
    pub fn temp<R>(f: impl for<'fh2> FnOnce(FrozenHeap<'fh2>) -> R) -> R {
        OwnedFrozenHeap::new().with(f)
    }

    #[cfg(test)]
    pub(crate) fn referenced_heaps(self) -> Vec<OwnedFrozen<()>> {
        self.0.refs.to_vec()
    }

    /// Keep the argument heap alive as long as this heap is kept alive. Used if a value in this
    /// heap points at values in another frozen heap.
    pub fn add_reference(self, heap: OwnedFrozenRef<'_, ()>) {
        self.0.refs.insert(heap);
    }

    pub(in crate::values::layout) fn string_interner(
        self,
    ) -> RefMut<'fh, StringValueInterner<'static>> {
        self.0.str_interner.borrow_mut()
    }

    pub(in crate::values::layout) fn alloc_raw<T>(
        self,
        x: AValueImpl<'fh, T>,
    ) -> ValueTyped<'fh, T::StarlarkValue>
    where
        T: AValue<'fh, ExtraElem = ()>,
        T::StarlarkValue: HeapSendable<'fh>,
        T::StarlarkValue: HeapSyncable<'fh>,
    {
        let v = self.0.arena.alloc(x);
        // SAFETY: just allocated in this heap, by the allocation's pointer.
        unsafe { ValueTyped::new_frozen_repr(v) }
    }

    pub(in crate::values::layout) fn alloc_raw_extra<T>(
        self,
        x: AValueImpl<'fh, T>,
    ) -> (
        ValueTyped<'fh, T::StarlarkValue>,
        *mut [MaybeUninit<T::ExtraElem>],
    )
    where
        T: AValue<'fh>,
        T::StarlarkValue: HeapSendable<'fh>,
        T::StarlarkValue: HeapSyncable<'fh>,
    {
        let (v, extra) = self.0.arena.alloc_extra(x);
        // SAFETY: just allocated in this heap, by the allocation's pointer.
        let v = unsafe { ValueTyped::new_frozen_repr(NonNull::new_unchecked(v)) };
        (v, extra)
    }

    #[inline]
    pub(in crate::values::layout) fn alloc_str_init(
        self,
        len: usize,
        hash: StarlarkHashValue,
        init: impl FnOnce(*mut u8),
    ) -> StringValue<'fh> {
        let v = self.0.arena.alloc_str_init(len, hash, init);

        // SAFETY: just allocated in this heap, by the allocation's pointer.
        unsafe {
            StringValue::new_unchecked(Value::new_frozen_ptr(NonNull::new_unchecked(v), true))
        }
    }

    /// Number of bytes allocated on this heap, not including any memory
    /// allocated outside of the starlark heap.
    pub fn allocated_bytes(self) -> usize {
        self.0.arena.allocated_bytes()
    }

    /// Number of bytes allocated by the heap but not yet filled.
    pub fn available_bytes(self) -> usize {
        self.0.arena.available_bytes()
    }

    /// Obtain a summary of how much memory is currently allocated by this heap.
    pub fn allocated_summary(self) -> HeapSummary {
        self.0.arena.allocated_summary()
    }

    pub(crate) fn reserve_with_extra<T>(
        self,
        extra_len: usize,
    ) -> (FrozenReservation<'fh, T>, *mut [MaybeUninit<T::ExtraElem>])
    where
        T: AValue<'fh>,
        T::StarlarkValue: HeapSendable<'fh>,
        T::StarlarkValue: HeapSyncable<'fh>,
    {
        let (r, extra) = self.0.arena.reserve_with_extra::<T>(extra_len);
        (FrozenReservation(r), extra)
    }
}

/// The frozen heaps a heap depends on.
///
/// A module's value heap and the frozen heap it is building hold one set between them, see
/// `ModuleHeaps`; every other heap has a set of its own. The two heaps of a module are used from
/// one thread; the lock, rather than a `RefCell`, only keeps the owned heaps `Send` (see
/// `HeapSendable`).
#[derive(Clone, Dupe, Default)]
pub(in crate::values::layout::heap) struct HeapReferences(Arc<Mutex<SmallSet<OwnedFrozen<()>>>>);

impl HeapReferences {
    pub(in crate::values::layout::heap) fn lock(
        &self,
    ) -> MutexGuard<'_, SmallSet<OwnedFrozen<()>>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(in crate::values::layout::heap) fn insert(&self, heap: OwnedFrozenRef<'_, ()>) {
        // The empty heap keeps nothing alive.
        if heap.heap_arc().is_empty() {
            return;
        }
        let mut refs = self.lock();
        // The same heap is added over and over (once per value brought out of it), so the lookup
        // saves the refcount round trip of `to_owned` in the common case.
        if !refs.contains(&heap) {
            refs.insert(heap.to_owned());
        }
    }

    /// The number of heaps, or `None` while the set is locked: for `Debug`, which must not block.
    pub(in crate::values::layout::heap) fn try_len(&self) -> Option<usize> {
        match self.0.try_lock() {
            Ok(refs) => Some(refs.len()),
            Err(TryLockError::Poisoned(e)) => Some(e.into_inner().len()),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    pub(in crate::values::layout::heap) fn to_vec(&self) -> Vec<OwnedFrozen<()>> {
        self.lock().iter().duped().collect()
    }
}
