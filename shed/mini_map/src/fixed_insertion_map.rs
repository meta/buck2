/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! [`MiniFixedInsertionMap`], a map whose keys are fixed at construction and iterate in
//! insertion order.
//!
//! Two layers live here. [`MiniFixedInsertionMap`] owns its allocation and is what most callers
//! want. Underneath it, [`layout_of`] + [`init`] + [`MiniFixedInsertionMapView`] are the same map
//! over memory somebody else owns.

use std::alloc::Layout;
use std::fmt;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::marker::PhantomData;
use std::mem;
use std::ptr;
use std::ptr::NonNull;

use allocative::Allocative;
use equivalent::Equivalent;
use mini_vec::packed_ptr::PackedPtr;
#[cfg(feature = "pagable_dep")]
use pagable::PagableDeserialize;
#[cfg(feature = "pagable_dep")]
use pagable::PagableSerialize;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::MiniMutInsertionMap;
use crate::arrays::DeallocGuard;
use crate::arrays::InitGuard;
use crate::arrays::alloc_block;
use crate::hashed::Hashed;
use crate::insertion_map::layout;
use crate::insertion_map::layout::Header;
use crate::insertion_map::raw::Raw;
use crate::mut_insertion_map::iter::OwnedEntries;

/// The iterators, shared with [`MiniMutInsertionMap`].
pub mod iter {
    pub use crate::mut_insertion_map::IntoIter;
    pub use crate::mut_insertion_map::IntoIterHashed;
    pub use crate::mut_insertion_map::IntoKeys;
    pub use crate::mut_insertion_map::IntoValues;
    pub use crate::mut_insertion_map::Iter;
    pub use crate::mut_insertion_map::IterHashed;
    pub use crate::mut_insertion_map::IterMut;
    pub use crate::mut_insertion_map::Keys;
    pub use crate::mut_insertion_map::Values;
    pub use crate::mut_insertion_map::ValuesMut;
}

use crate::fixed_insertion_map::iter::IntoIter;
use crate::fixed_insertion_map::iter::IntoIterHashed;
use crate::fixed_insertion_map::iter::IntoKeys;
use crate::fixed_insertion_map::iter::IntoValues;
use crate::fixed_insertion_map::iter::Iter;
use crate::fixed_insertion_map::iter::IterHashed;
use crate::fixed_insertion_map::iter::IterMut;
use crate::fixed_insertion_map::iter::Keys;
use crate::fixed_insertion_map::iter::Values;
use crate::fixed_insertion_map::iter::ValuesMut;

/// Packed-pointer metadata meaning "the entry count is in the heap header".
///
/// Small maps store their length here instead, which is why they need no header at all.
const LARGE_LEN: u8 = u8::MAX;

const _: () = assert!(layout::NO_INDEX_THRESHOLD < LARGE_LEN as usize);

/// Whether a map of `n` entries carries a header, which it does exactly when it carries an index.
#[inline]
fn is_large(n: usize) -> bool {
    layout::has_index(n)
}

#[inline]
fn offsets<K, V>(n: usize) -> layout::Offsets<K, V> {
    layout::offsets::<K, V>(n, is_large(n))
}

/// How big an allocation a fixed insertion map of `n` entries needs, and how it must be aligned.
///
/// A zero-sized layout for `n == 0`: an empty map does not allocate. Panics if `n` is large enough
/// that the layout overflows.
pub fn layout_of<K, V>(n: usize) -> Layout {
    layout::checked_offsets::<K, V>(n, is_large(n)).layout
}

/// Write a fixed insertion map of pre-hashed, unique entries into `ptr`, in the order given.
///
/// # Safety
///
/// `ptr` must be valid for writes of [`layout_of::<K, V>(entries.len())`](layout_of) bytes, with
/// that layout's alignment, and its contents are treated as uninitialized. `entries` must yield
/// exactly `len()` items and no two of them may have equal keys; a duplicate is stored, iterated,
/// and reachable by lookup or not depending on which copy the index happens to find first.
///
/// On success `ptr` holds an initialized map, and the caller owns the keys and values in it. If
/// `entries` panics, every entry written so far is dropped and `ptr` is uninitialized again; the
/// caller is responsible for freeing it.
pub unsafe fn init<K, V>(ptr: NonNull<u8>, entries: impl ExactSizeIterator<Item = (Hashed<K>, V)>) {
    let len = entries.len();
    unsafe { init_with_len(ptr, len, entries) }
}

/// [`init`] with the length taken from the caller, which has already sized the allocation by it.
///
/// # Safety
///
/// As [`init`], with `len` standing in for `entries.len()`.
pub(crate) unsafe fn init_with_len<K, V>(
    ptr: NonNull<u8>,
    len: usize,
    mut entries: impl ExactSizeIterator<Item = (Hashed<K>, V)>,
) {
    if len == 0 {
        return;
    }
    // SAFETY: the caller's allocation has the layout of `len` entries.
    let raw = unsafe { Raw::new(ptr, len, is_large(len)) };
    let arrays = raw.arrays();
    let mut guard = InitGuard::new(arrays);

    // Trusting `len` only for the size of the allocation, never for how many items to pull, is
    // what keeps a misbehaving `ExactSizeIterator` from writing past it.
    for i in 0..len {
        let Some((key, value)) = entries.next() else {
            panic!("iterator yielded fewer than its {len} items");
        };
        // SAFETY: `i < len`, and this slot has not been written yet.
        unsafe { arrays.write(i, key.hash().get(), key.into_key(), value) };
        guard.written = i + 1;
    }
    assert!(
        entries.next().is_none(),
        "iterator yielded more than its {len} items"
    );

    // SAFETY: all `len` entries are written; nothing below can panic.
    unsafe { finish(raw, len) };
    mem::forget(guard);
}

/// Write the header and index of a large map whose entries are in place. Cannot panic.
unsafe fn finish<K, V>(raw: Raw<K, V>, len: usize) {
    if is_large(len) {
        unsafe {
            ptr::write(raw.header(), Header { len, cap: len });
            raw.index_rebuild(len);
        }
    }
}

/// A map whose keys are fixed at construction and iterate in insertion order.
///
/// The frozen form of [`MiniMutInsertionMap`]: the same entries, order, stored hashes and index,
/// with the spare capacity gone, in a single allocation behind a one-word handle; an empty map does
/// not allocate at all. No key can be added, removed or moved afterwards, while values can be
/// replaced in place through [`get_mut`](Self::get_mut), [`iter_mut`](Self::iter_mut) and
/// [`values_mut`](Self::values_mut). Positions are part of what the type models, so it exposes
/// them: [`get_index`](Self::get_index) and [`get_index_of`](Self::get_index_of).
///
/// `==` and `Hash` are order-sensitive, as the name promises.
#[repr(transparent)]
pub struct MiniFixedInsertionMap<K, V> {
    /// Points at the allocation described by [`layout_of`]; dangling when empty. The packed byte
    /// is the entry count, or [`LARGE_LEN`] when that does not fit.
    raw: PackedPtr<NonNull<u8>>,
    _marker: PhantomData<(K, V)>,
}

// SAFETY: the map owns its keys and values outright, so it can cross threads exactly when they can.
unsafe impl<K: Send, V: Send> Send for MiniFixedInsertionMap<K, V> {}
// SAFETY: `&MiniFixedInsertionMap` hands out `&K`/`&V` and nothing else.
unsafe impl<K: Sync, V: Sync> Sync for MiniFixedInsertionMap<K, V> {}

impl<K, V> MiniFixedInsertionMap<K, V> {
    /// An empty map. Does not allocate.
    #[inline]
    pub fn new() -> Self {
        MiniFixedInsertionMap {
            raw: PackedPtr::new(NonNull::dangling(), 0),
            _marker: PhantomData,
        }
    }

    #[inline]
    fn base(&self) -> NonNull<u8> {
        self.raw.as_ref()
    }

    /// The handle for an allocation that holds an initialized map of `len > 0` entries.
    ///
    /// # Safety
    ///
    /// `ptr` must have been laid out by [`layout_of`] for `len` entries and initialized as
    /// [`init`] would, and nothing else may free or refer to it.
    pub(crate) unsafe fn from_raw_parts(ptr: NonNull<u8>, len: usize) -> Self {
        debug_assert!(len > 0);
        let extra = if is_large(len) { LARGE_LEN } else { len as u8 };
        MiniFixedInsertionMap {
            raw: PackedPtr::new(ptr, extra),
            _marker: PhantomData,
        }
    }

    /// Number of entries.
    #[inline]
    pub fn len(&self) -> usize {
        match self.raw.extra() {
            LARGE_LEN => {
                // SAFETY: the packed byte says this allocation is in the large regime, so it
                // starts with the header.
                let header = unsafe { *self.base().as_ptr().cast::<Header>() };
                header.len
            }
            small => small as usize,
        }
    }

    /// `true` iff there are no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.raw.extra() == 0
    }

    /// Borrow the map as a view, the type that carries the read API.
    #[inline]
    pub fn view(&self) -> MiniFixedInsertionMapView<'_, K, V> {
        // SAFETY: we own an allocation written by `init` for this `K`, `V` and length.
        unsafe { MiniFixedInsertionMapView::from_raw(self.base(), self.len()) }
    }

    /// Build from pre-hashed entries whose keys are already known to be unique, in this order.
    ///
    /// See [`init`] for what a duplicate does.
    pub fn from_hashed_unique(entries: impl ExactSizeIterator<Item = (Hashed<K>, V)>) -> Self {
        let len = entries.len();
        if len == 0 {
            return Self::new();
        }
        let layout = layout_of::<K, V>(len);
        let ptr = alloc_block(layout);
        let guard = DeallocGuard { ptr, layout };
        // SAFETY: freshly allocated with the layout for `len`. On a panic `init_with_len` has
        // dropped whatever it wrote, so freeing the block is all that is left to do.
        unsafe { init_with_len(ptr, len, entries) };
        mem::forget(guard);
        // SAFETY: just initialized for `len` entries.
        unsafe { Self::from_raw_parts(ptr, len) }
    }

    /// Look up a key whose hash has already been computed.
    #[inline(always)]
    pub fn get_hashed<Q>(&self, key: Hashed<&Q>) -> Option<&V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.view().get_hashed(key)
    }

    /// Look up a key.
    #[inline(always)]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.view().get(key)
    }

    /// Look up a key, returning its position and the stored key alongside its value.
    #[inline(always)]
    pub fn get_full<Q>(&self, key: &Q) -> Option<(usize, &K, &V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.view().get_full(key)
    }

    /// Look up a key whose hash has already been computed, returning its position and the stored
    /// key alongside its value.
    #[inline(always)]
    pub fn get_full_hashed<Q>(&self, key: Hashed<&Q>) -> Option<(usize, &K, &V)>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.view().get_full_hashed(key)
    }

    /// The entry at a position.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        self.view().get_index(index)
    }

    /// The position of a key.
    #[inline(always)]
    pub fn get_index_of<Q>(&self, key: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.view().get_index_of(key)
    }

    /// The position of a key whose hash has already been computed.
    #[inline(always)]
    pub fn get_index_of_hashed<Q>(&self, key: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.view().get_index_of_hashed(key)
    }

    /// Whether the map holds `key`.
    #[inline(always)]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.view().contains_key(key)
    }

    /// Whether the map holds `key`, whose hash has already been computed.
    #[inline]
    pub fn contains_key_hashed<Q>(&self, key: Hashed<&Q>) -> bool
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.view().contains_key_hashed(key)
    }

    /// Iterate the entries in insertion order.
    #[inline]
    pub fn iter(&self) -> Iter<'_, K, V> {
        self.view().iter()
    }

    /// Iterate the entries with their stored hashes, so a rebuild does not have to hash again.
    #[inline]
    pub fn iter_hashed(&self) -> IterHashed<'_, K, V> {
        self.view().iter_hashed()
    }

    /// Iterate the keys in insertion order.
    #[inline]
    pub fn keys(&self) -> Keys<'_, K, V> {
        self.view().keys()
    }

    /// Iterate the values in insertion order.
    #[inline]
    pub fn values(&self) -> Values<'_, K, V> {
        self.view().values()
    }

    /// The first entry.
    #[inline]
    pub fn first(&self) -> Option<(&K, &V)> {
        self.view().first()
    }

    /// The last entry.
    #[inline]
    pub fn last(&self) -> Option<(&K, &V)> {
        self.view().last()
    }

    /// Look up a key whose hash has already been computed, for writing to its value.
    #[inline(always)]
    pub fn get_mut_hashed<Q>(&mut self, key: Hashed<&Q>) -> Option<&mut V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let view = self.view();
        let i = view.get_index_of_hashed(key)?;
        // SAFETY: `i` is the position of an entry, borrowed exclusively through `&mut self`.
        Some(unsafe { view.raw.arrays().value_mut(i) })
    }

    /// Look up a key, for writing to its value.
    #[inline(always)]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_mut_hashed(Hashed::new(key))
    }

    /// Iterate the entries in insertion order, with the values writable.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        let view = self.view();
        let arrays = view.raw.arrays();
        // SAFETY: the arrays hold `len` initialized entries, `&mut self` makes the borrow
        // exclusive, and keys and values are disjoint regions.
        unsafe { IterMut::new(arrays.keys(view.len), arrays.values_mut(view.len)) }
    }

    /// Iterate the values in insertion order, writable.
    #[inline]
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        let view = self.view();
        // SAFETY: as in `iter_mut`.
        ValuesMut::new(unsafe { view.raw.arrays().values_mut(view.len) })
    }

    fn into_owned_entries(self) -> OwnedEntries<K, V> {
        let view = self.view();
        let (arrays, len) = (view.raw.arrays(), view.len);
        let block = if len == 0 {
            None
        } else {
            Some((view.raw.base(), view.raw.layout()))
        };
        mem::forget(self);
        // SAFETY: the map held `len` entries in these arrays and is forgotten, so the iterator is
        // their only owner, and `block` is the allocation they are in.
        unsafe { OwnedEntries::new(arrays, len, block) }
    }

    /// Consume the map into its entries with their stored hashes, in insertion order.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<K, V> {
        IntoIterHashed::new(self.into_owned_entries())
    }

    /// Consume the map into its keys, in insertion order.
    #[inline]
    pub fn into_keys(self) -> IntoKeys<K, V> {
        IntoKeys::new(self.into_owned_entries())
    }

    /// Consume the map into its values, in insertion order.
    #[inline]
    pub fn into_values(self) -> IntoValues<K, V> {
        IntoValues::new(self.into_owned_entries())
    }

    #[cfg(test)]
    pub(crate) fn assert_invariants(&self) {
        self.view().assert_invariants();
        assert_eq!(self.len(), self.iter().count());
    }
}

impl<K, V> Default for MiniFixedInsertionMap<K, V> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Debug, V: Debug> Debug for MiniFixedInsertionMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> MiniFixedInsertionMap<K, V> {
    fn drop_impl(&mut self) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let view = self.view();
        let _guard = DeallocGuard {
            ptr: self.base(),
            layout: view.raw.layout(),
        };
        // SAFETY: the arrays hold `len` initialized entries that this map owns.
        unsafe { view.raw.arrays().drop_entries(len) };
    }
}

// SAFETY: `may_dangle` lets a map holding borrowed data be dropped after that data, as `Vec`
// allows, because dropping touches the entries only to drop them; see the Rustonomicon on drop
// check. It is nightly-only, so without `rust_nightly` a plain `Drop` impl applies with the stricter
// rules that come with it.
#[cfg(rust_nightly)]
unsafe impl<#[may_dangle] K, #[may_dangle] V> Drop for MiniFixedInsertionMap<K, V> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

#[cfg(not(rust_nightly))]
impl<K, V> Drop for MiniFixedInsertionMap<K, V> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

impl<K: Clone, V: Clone> Clone for MiniFixedInsertionMap<K, V> {
    fn clone(&self) -> Self {
        let len = self.len();
        if len == 0 {
            return Self::new();
        }
        let layout = layout_of::<K, V>(len);
        let ptr = alloc_block(layout);
        let dealloc = DeallocGuard { ptr, layout };
        // SAFETY: freshly allocated with this layout.
        let new = unsafe { Raw::new(ptr, len, is_large(len)) };
        let mut guard = InitGuard::new(new.arrays());
        for (i, (key, value)) in self.iter_hashed().enumerate() {
            let hash = key.hash().get();
            // Clone both before writing either, so that a panic in the second clone drops the
            // first by unwinding rather than stranding it in the destination.
            let key = K::clone(key.key());
            let value = value.clone();
            // SAFETY: `i < len`, and this slot has not been written yet.
            unsafe { new.arrays().write(i, hash, key, value) };
            guard.written = i + 1;
        }
        // SAFETY: all `len` entries are written.
        unsafe { finish(new, len) };
        mem::forget(guard);
        mem::forget(dealloc);
        // SAFETY: just initialized for `len` entries.
        unsafe { Self::from_raw_parts(ptr, len) }
    }
}

/// Order-sensitive: the same entries in the same order.
impl<K: PartialEq, V: PartialEq> PartialEq for MiniFixedInsertionMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        let (this, that) = (self.view(), other.view());
        this.len() == that.len()
            // The stored hashes settle almost every inequality without touching a key.
            && this.hashes_slice() == that.hashes_slice()
            && this.keys_slice() == that.keys_slice()
            && this.values_slice() == that.values_slice()
    }
}

impl<K: Eq, V: Eq> Eq for MiniFixedInsertionMap<K, V> {}

/// Order-sensitive, and the same as [`MiniMutInsertionMap::hash_ordered`] of the map this was
/// frozen from, so freezing never changes a hash. Keys are not rehashed; their stored hashes
/// stand in.
impl<K: Hash, V: Hash> Hash for MiniFixedInsertionMap<K, V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for entry in self.iter_hashed() {
            entry.hash(state);
        }
    }
}

impl<K: StrongHash, V: StrongHash> StrongHash for MiniFixedInsertionMap<K, V> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.len().strong_hash(state);
        for (key, value) in self.iter() {
            key.strong_hash(state);
            value.strong_hash(state);
        }
    }
}

impl<K: Allocative, V: Allocative> Allocative for MiniFixedInsertionMap<K, V> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        let len = self.len();
        if len > 0 {
            let view = self.view();
            let offsets = offsets::<K, V>(len);
            let mut visitor =
                visitor.enter_unique(allocative::Key::new("ptr"), size_of::<*const u8>());
            let header = if is_large(len) {
                visitor.visit_simple(allocative::Key::new("header"), size_of::<Header>());
                size_of::<Header>()
            } else {
                0
            };
            visitor.visit_simple(allocative::Key::new("hashes"), size_of::<u32>() * len);
            {
                let mut visitor = visitor.enter(allocative::Key::new("keys"), size_of::<K>() * len);
                visitor.visit_slice::<K>(view.keys_slice());
                visitor.exit();
            }
            {
                let mut visitor =
                    visitor.enter(allocative::Key::new("values"), size_of::<V>() * len);
                visitor.visit_slice::<V>(view.values_slice());
                visitor.exit();
            }
            let index = offsets.layout.size() - offsets.index;
            if index > 0 {
                visitor.visit_simple(allocative::Key::new("index"), index);
            }
            // Whatever `Layout::extend` inserted between the regions, so the reported size is the
            // size of the real allocation.
            let accounted =
                header + (size_of::<u32>() + size_of::<K>() + size_of::<V>()) * len + index;
            let padding = offsets.layout.size() - accounted;
            if padding > 0 {
                visitor.visit_simple(allocative::Key::new("padding"), padding);
            }
            visitor.exit();
        }
        visitor.exit();
    }
}

impl<K: Hash + Eq, V> FromIterator<(K, V)> for MiniFixedInsertionMap<K, V> {
    /// A repeated key keeps its first position and its last value, as a sequence of inserts would.
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        MiniMutInsertionMap::from_iter(iter).freeze()
    }
}

impl<K: Eq, V> FromIterator<(Hashed<K>, V)> for MiniFixedInsertionMap<K, V> {
    /// A repeated key keeps its first position and its last value, as a sequence of inserts would.
    fn from_iter<I: IntoIterator<Item = (Hashed<K>, V)>>(iter: I) -> Self {
        MiniMutInsertionMap::from_iter(iter).freeze()
    }
}

impl<'a, K, V> IntoIterator for &'a MiniFixedInsertionMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    #[inline]
    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut MiniFixedInsertionMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    #[inline]
    fn into_iter(self) -> IterMut<'a, K, V> {
        self.iter_mut()
    }
}

impl<K, V> IntoIterator for MiniFixedInsertionMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    #[inline]
    fn into_iter(self) -> IntoIter<K, V> {
        IntoIter::new(self.into_owned_entries())
    }
}

impl<K: Serialize, V: Serialize> Serialize for MiniFixedInsertionMap<K, V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.iter())
    }
}

impl<'de, K, V> Deserialize<'de> for MiniFixedInsertionMap<K, V>
where
    K: Deserialize<'de> + Hash + Eq,
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        MiniMutInsertionMap::deserialize(deserializer).map(MiniMutInsertionMap::freeze)
    }
}

#[cfg(feature = "pagable_dep")]
impl<K: pagable::Pagable, V: pagable::Pagable> PagableSerialize for MiniFixedInsertionMap<K, V> {
    /// The same encoding as [`MiniMutInsertionMap`]: the length, then each entry's key, value and
    /// stored hash in iteration order.
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        usize::serialize(&self.len(), serializer.serde())?;
        for (key, value) in self.iter_hashed() {
            key.key().pagable_serialize(serializer)?;
            value.pagable_serialize(serializer)?;
            key.hash().pagable_serialize(serializer)?;
        }
        Ok(())
    }
}

#[cfg(feature = "pagable_dep")]
impl<'de, K: pagable::Pagable, V: pagable::Pagable> PagableDeserialize<'de>
    for MiniFixedInsertionMap<K, V>
{
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        Ok(MiniMutInsertionMap::pagable_deserialize(deserializer)?.freeze())
    }
}

/// A read-only fixed insertion map borrowed out of memory somebody else owns.
///
/// This is the whole read API of [`MiniFixedInsertionMap`]; the owned type is a pointer plus a
/// `Drop` that delegates here. Callers that want such a map inside an allocation of their own
/// size that allocation with [`layout_of`], fill it with [`init`], and read it back through this
/// type.
///
/// The entry count is not stored in the allocation in the small regime, so it is the caller's job
/// to remember it and hand it back to [`from_raw`](Self::from_raw).
pub struct MiniFixedInsertionMapView<'a, K, V> {
    raw: Raw<K, V>,
    len: usize,
    _marker: PhantomData<(&'a [K], &'a [V])>,
}

// SAFETY: a view is a shared borrow of the map, so it crosses threads exactly when `&K`/`&V` do.
unsafe impl<K: Sync, V: Sync> Send for MiniFixedInsertionMapView<'_, K, V> {}
// SAFETY: as above; a view hands out `&K`/`&V` and nothing else.
unsafe impl<K: Sync, V: Sync> Sync for MiniFixedInsertionMapView<'_, K, V> {}

impl<K, V> Clone for MiniFixedInsertionMapView<'_, K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, V> Copy for MiniFixedInsertionMapView<'_, K, V> {}

impl<'a, K, V> MiniFixedInsertionMapView<'a, K, V> {
    /// A view over an empty map. Reads no memory.
    #[inline]
    pub fn empty() -> Self {
        // SAFETY: `len == 0` means no region is ever read, so a dangling pointer is fine.
        unsafe { Self::from_raw(NonNull::dangling(), 0) }
    }

    /// Borrow a fixed insertion map out of raw memory.
    ///
    /// # Safety
    ///
    /// `ptr` must point at an allocation laid out by [`init`] for the same `K`, `V` and `len`,
    /// whose entries are still initialized and which outlives `'a`. When `len` is 0, `ptr` is not
    /// read and may dangle.
    #[inline]
    pub unsafe fn from_raw(ptr: NonNull<u8>, len: usize) -> Self {
        let raw = if len == 0 {
            Raw::dangling()
        } else {
            // SAFETY: the caller's allocation has the layout of `len` entries.
            unsafe { Raw::new(ptr, len, is_large(len)) }
        };
        MiniFixedInsertionMapView {
            raw,
            len,
            _marker: PhantomData,
        }
    }

    /// Number of entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// `true` iff there are no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    pub(crate) fn keys_slice(&self) -> &'a [K] {
        // SAFETY: `from_raw`'s contract says `len` entries are initialized and outlive `'a`.
        unsafe { self.raw.arrays().keys(self.len) }
    }

    #[inline]
    pub(crate) fn values_slice(&self) -> &'a [V] {
        // SAFETY: as in `keys_slice`.
        unsafe { self.raw.arrays().values(self.len) }
    }

    /// The stored short hashes, parallel to the key and value arrays.
    #[inline]
    pub(crate) fn hashes_slice(&self) -> &'a [u32] {
        // SAFETY: as in `keys_slice`.
        unsafe { self.raw.arrays().hashes(self.len) }
    }

    /// Position of `key` in the entry arrays.
    #[inline(always)]
    fn find_index<Q>(&self, key: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let arrays = self.raw.arrays();
        // SAFETY: `len` entries are initialized, and `find` only hands out their positions.
        unsafe {
            self.raw.find(self.len, key.hash().get(), move |i| {
                key.key().equivalent(arrays.key(i))
            })
        }
    }

    /// Look up a key whose hash has already been computed.
    #[inline(always)]
    pub fn get_hashed<Q>(&self, key: Hashed<&Q>) -> Option<&'a V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let i = self.find_index(key)?;
        Some(&self.values_slice()[i])
    }

    /// Look up a key.
    #[inline(always)]
    pub fn get<Q>(&self, key: &Q) -> Option<&'a V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_hashed(Hashed::new(key))
    }

    /// Look up a key whose hash has already been computed, returning its position and the stored
    /// key alongside its value.
    #[inline(always)]
    pub fn get_full_hashed<Q>(&self, key: Hashed<&Q>) -> Option<(usize, &'a K, &'a V)>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let i = self.find_index(key)?;
        Some((i, &self.keys_slice()[i], &self.values_slice()[i]))
    }

    /// Look up a key, returning its position and the stored key alongside its value.
    #[inline(always)]
    pub fn get_full<Q>(&self, key: &Q) -> Option<(usize, &'a K, &'a V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_full_hashed(Hashed::new(key))
    }

    /// The entry at a position.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<(&'a K, &'a V)> {
        if index < self.len {
            Some((&self.keys_slice()[index], &self.values_slice()[index]))
        } else {
            None
        }
    }

    /// The position of a key.
    #[inline(always)]
    pub fn get_index_of<Q>(&self, key: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.find_index(Hashed::new(key))
    }

    /// The position of a key whose hash has already been computed.
    #[inline(always)]
    pub fn get_index_of_hashed<Q>(&self, key: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.find_index(key)
    }

    /// Whether the map holds `key`.
    #[inline(always)]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_index_of(key).is_some()
    }

    /// Whether the map holds `key`, whose hash has already been computed.
    #[inline]
    pub fn contains_key_hashed<Q>(&self, key: Hashed<&Q>) -> bool
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.find_index(key).is_some()
    }

    /// Iterate the entries in insertion order.
    #[inline]
    pub fn iter(&self) -> Iter<'a, K, V> {
        Iter::new(self.keys_slice(), self.values_slice())
    }

    /// Iterate the entries with their stored hashes, so a rebuild does not have to hash again.
    #[inline]
    pub fn iter_hashed(&self) -> IterHashed<'a, K, V> {
        IterHashed::new(self.keys_slice(), self.values_slice(), self.hashes_slice())
    }

    /// Iterate the keys in insertion order.
    #[inline]
    pub fn keys(&self) -> Keys<'a, K, V> {
        Keys::new(self.keys_slice())
    }

    /// Iterate the values in insertion order.
    #[inline]
    pub fn values(&self) -> Values<'a, K, V> {
        Values::new(self.values_slice())
    }

    /// The first entry.
    #[inline]
    pub fn first(&self) -> Option<(&'a K, &'a V)> {
        self.get_index(0)
    }

    /// The last entry.
    #[inline]
    pub fn last(&self) -> Option<(&'a K, &'a V)> {
        self.get_index(self.len.checked_sub(1)?)
    }

    /// Everything the index stores, for the test that equal maps are byte-identical.
    #[cfg(test)]
    pub(crate) fn index_contents(&self) -> (Vec<u8>, Vec<usize>) {
        if self.len == 0 {
            return (Vec::new(), Vec::new());
        }
        // SAFETY: the allocation has the layout the view was made with.
        unsafe { self.raw.index_contents() }
    }

    /// Check every invariant the lookup relies on.
    #[cfg(test)]
    pub(crate) fn assert_invariants(&self) {
        // SAFETY: `len` entries are initialized.
        unsafe { self.raw.assert_index_consistent(self.len) };
        if is_large(self.len) {
            // SAFETY: large maps have a header.
            let header = unsafe { *self.raw.header() };
            assert_eq!(self.len, header.len);
            assert_eq!(self.len, header.cap);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::alloc;
    use std::rc::Rc;

    use super::*;

    /// A key whose stored hash we choose, for building the layouts a real hasher would not.
    fn forced<K>(hash: u32, key: K) -> Hashed<K> {
        Hashed::new_unchecked(crate::ShortHash::new_unchecked(hash), key)
    }

    fn map_of(n: u32) -> MiniFixedInsertionMap<u32, u32> {
        (0..n).map(|i| (i, i * 10)).collect()
    }

    /// Values change in place; keys, positions and stored hashes do not.
    #[test]
    fn values_are_writable_in_both_regimes() {
        for n in [0u32, 1, 5, 32, 33, 100] {
            let mut map = map_of(n);
            for i in 0..n {
                *map.get_mut(&i).unwrap() += 1;
            }
            assert_eq!(None, map.get_mut(&n));
            for (k, v) in map.iter_mut() {
                *v += *k;
            }
            for v in map.values_mut() {
                *v *= 2;
            }
            for (k, v) in &mut map {
                *v -= *k;
            }
            map.assert_invariants();
            let fresh = map_of(n);
            assert!(map.keys().eq(fresh.keys()));
            assert!(
                map.iter_hashed()
                    .map(|(k, _)| k.hash())
                    .eq(fresh.iter_hashed().map(|(k, _)| k.hash()))
            );
            for i in 0..n {
                assert_eq!(Some(&((i * 11 + 1) * 2 - i)), map.get(&i), "n={n} i={i}");
                assert_eq!(Some(i as usize), map.get_index_of(&i));
            }
        }
    }

    /// Owned iteration hands out the entries in order; whatever is not taken is dropped with the
    /// iterator, and so is the allocation.
    #[test]
    fn into_iter_moves_the_entries_out() {
        for n in [0usize, 1, 5, 32, 33, 100] {
            let token = Rc::new(());
            let map: MiniFixedInsertionMap<usize, Rc<()>> =
                (0..n).map(|i| (i, token.clone())).collect();
            assert_eq!(n + 1, Rc::strong_count(&token));
            let mut iter = map.into_iter();
            assert_eq!(n, iter.len());
            let taken: Vec<(usize, Rc<()>)> = iter.by_ref().take(n / 2).collect();
            assert!(taken.iter().map(|(k, _)| *k).eq(0..n / 2));
            drop(iter);
            assert_eq!(n / 2 + 1, Rc::strong_count(&token));
            drop(taken);
            assert_eq!(1, Rc::strong_count(&token));
        }

        let fresh = map_of(40);
        assert!(map_of(40).into_keys().eq(0..40));
        assert!(map_of(40).into_values().eq((0..40).map(|i| i * 10)));
        assert!(
            map_of(40)
                .into_iter()
                .rev()
                .eq(fresh.iter().rev().map(|(k, v)| (*k, *v)))
        );
        assert!(
            map_of(40)
                .into_iter_hashed()
                .map(|(k, v)| (k.hash(), *k.key(), v))
                .eq(fresh.iter_hashed().map(|(k, v)| (k.hash(), **k.key(), *v)))
        );
    }

    #[test]
    fn empty_does_not_allocate() {
        let map = MiniFixedInsertionMap::<u64, u64>::new();
        assert_eq!(0, map.len());
        assert!(map.is_empty());
        assert_eq!(None, map.get(&1));
        assert_eq!(None, map.first());
        assert_eq!(0, map.iter().count());
        assert_eq!(0, layout_of::<u64, u64>(0).size());
    }

    /// Every size from empty through well past the index threshold, hits, misses and positions.
    #[test]
    fn lookups_across_both_regimes() {
        for n in 0..200u32 {
            let map = map_of(n);
            map.assert_invariants();
            assert_eq!(n as usize, map.len());
            for i in 0..n {
                assert_eq!(Some(&(i * 10)), map.get(&i), "n={n} i={i}");
                assert_eq!(Some(i as usize), map.get_index_of(&i));
                assert_eq!(Some((&i, &(i * 10))), map.get_index(i as usize));
                assert_eq!(Some((i as usize, &i, &(i * 10))), map.get_full(&i));
                assert!(map.contains_key(&i));
            }
            for i in n..n + 5 {
                assert_eq!(None, map.get(&i), "n={n} i={i}");
                assert_eq!(None, map.get_index(i as usize));
            }
            if n > 0 {
                assert_eq!(Some((&0, &0)), map.first());
                assert_eq!(Some((&(n - 1), &((n - 1) * 10))), map.last());
            }
        }
    }

    /// Insertion order is the iteration order and part of equality; the same entries in the same
    /// order are byte-identical however they were built.
    #[test]
    fn order_is_preserved_and_compared() {
        for n in [1u32, 5, 32, 33, 100, 500] {
            let forwards = map_of(n);
            let backwards: MiniFixedInsertionMap<u32, u32> =
                (0..n).rev().map(|i| (i, i * 10)).collect();
            assert_eq!(
                (0..n).collect::<Vec<_>>(),
                forwards.keys().copied().collect::<Vec<_>>()
            );
            assert_eq!(
                (0..n).rev().collect::<Vec<_>>(),
                backwards.keys().copied().collect::<Vec<_>>()
            );
            if n > 1 {
                assert_ne!(forwards, backwards, "n={n}");
            }

            let mut mutable = MiniMutInsertionMap::with_capacity(1000);
            for i in 0..n {
                mutable.insert(i, i * 10);
            }
            let frozen = mutable.freeze();
            assert_eq!(forwards, frozen, "n={n}");
            assert_eq!(forwards.view().hashes_slice(), frozen.view().hashes_slice());
            assert_eq!(
                forwards.view().index_contents(),
                frozen.view().index_contents()
            );
        }
    }

    #[test]
    fn freeze_never_changes_a_hash() {
        fn hash_of(value: &impl Hash) -> u64 {
            let mut hasher = crate::FastHasher::new();
            value.hash(&mut hasher);
            hasher.finish()
        }
        for n in [0u32, 1, 40, 300] {
            let mutable: MiniMutInsertionMap<String, u32> =
                (0..n).map(|i| (format!("k{i}"), i)).collect();
            let mut hasher = crate::FastHasher::new();
            mutable.hash_ordered(&mut hasher);
            let before = hasher.finish();
            assert_eq!(before, hash_of(&mutable));
            let frozen = mutable.freeze();
            assert_eq!(before, hash_of(&frozen), "n={n}");
        }
    }

    #[test]
    fn duplicate_keys_keep_first_position_and_last_value() {
        let map: MiniFixedInsertionMap<u64, u64> = [(1, 10), (2, 20), (1, 11), (3, 30), (1, 12)]
            .into_iter()
            .collect();
        assert_eq!(3, map.len());
        assert_eq!(Some((0, &1, &12)), map.get_full(&1));
        assert_eq!(Some(&20), map.get(&2));
        map.assert_invariants();
    }

    #[test]
    fn colliding_hashes_are_told_apart_by_key() {
        for n in [1u32, 5, 40, 200] {
            let map =
                MiniFixedInsertionMap::from_hashed_unique((0..n).map(|i| (forced(7, i), i * 2)));
            map.assert_invariants();
            for i in 0..n {
                assert_eq!(Some(&(i * 2)), map.get_hashed(forced(7, &i)), "n={n} i={i}");
            }
            assert_eq!(None, map.get_hashed(forced(7, &n)));
            assert_eq!(None, map.get_hashed(forced(8, &0)));
        }
    }

    #[test]
    fn string_keys_look_up_by_str() {
        let map: MiniFixedInsertionMap<String, u32> =
            (0..100u32).map(|i| (format!("key{i}"), i)).collect();
        map.assert_invariants();
        for i in 0..100u32 {
            assert_eq!(Some(&i), map.get(format!("key{i}").as_str()));
        }
        assert_eq!(None, map.get("nope"));
    }

    #[test]
    fn clone_is_an_independent_identical_map() {
        for n in [0u32, 1, 33, 300] {
            let map = map_of(n);
            let clone = map.clone();
            clone.assert_invariants();
            assert_eq!(map, clone);
            assert_eq!(map.view().index_contents(), clone.view().index_contents());
            if n > 0 {
                assert_ne!(
                    map.view().keys_slice().as_ptr(),
                    clone.view().keys_slice().as_ptr(),
                    "the clone must have its own allocation"
                );
            }
        }
    }

    #[test]
    fn auto_traits_follow_the_entries() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<MiniFixedInsertionMap<String, Vec<u8>>>();
        send_and_sync::<MiniFixedInsertionMapView<'static, String, Vec<u8>>>();
    }

    #[test]
    fn handle_is_one_word() {
        mini_vec::size_assert::words_of_type!(MiniFixedInsertionMap<u64, u64>, 1);
        mini_vec::size_assert::words_of_type!(MiniFixedInsertionMap<String, Vec<u8>>, 1);
    }

    #[test]
    fn serde_round_trips_in_order() {
        let map: MiniFixedInsertionMap<String, u32> =
            (0..50u32).rev().map(|i| (format!("k{i}"), i)).collect();
        let json = serde_json::to_string(&map).unwrap();
        assert!(json.starts_with(r#"{"k49":49,"k48":48"#), "{json}");
        let back: MiniFixedInsertionMap<String, u32> = serde_json::from_str(&json).unwrap();
        assert_eq!(map, back);
        back.assert_invariants();
    }

    #[cfg(feature = "pagable_dep")]
    #[test]
    fn pagable_round_trips_and_matches_the_mutable_map() {
        let mutable: MiniMutInsertionMap<u64, u64> = (0..100u64).map(|i| (i, i * 5)).collect();
        let bytes_of = |map: &dyn PagableSerialize| {
            let mut serializer = pagable::testing::TestingSerializer::new();
            map.pagable_serialize(&mut serializer).unwrap();
            serializer.finish()
        };
        let bytes = bytes_of(&mutable);
        let frozen = mutable.freeze();
        assert_eq!(bytes, bytes_of(&frozen));
        let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
        let back =
            MiniFixedInsertionMap::<u64, u64>::pagable_deserialize(&mut deserializer).unwrap();
        assert_eq!(frozen, back);
    }

    #[test]
    fn allocative_reports_the_whole_allocation() {
        for n in [1u32, 33, 300] {
            let map = map_of(n);
            let reported = allocative::size_of_unique_allocated_data(&map);
            assert_eq!(
                layout_of::<u32, u32>(n as usize).size(),
                reported,
                "n={n}: allocative must account for every byte of the allocation"
            );
        }
    }

    /// The view is the read API a caller gets over memory it owns itself.
    #[test]
    fn view_over_caller_owned_memory() {
        for n in [5usize, 100] {
            let layout = layout_of::<u64, u64>(n);
            // SAFETY: non-zero-sized layout; freed below after the entries are dropped.
            let ptr = NonNull::new(unsafe { alloc::alloc(layout) }).unwrap();
            // SAFETY: `ptr` has exactly this layout and the keys are unique.
            unsafe { init(ptr, (0..n).map(|i| (Hashed::new(i as u64), (i * 2) as u64))) };
            // SAFETY: just initialized for this `K`, `V` and length.
            let view = unsafe { MiniFixedInsertionMapView::<u64, u64>::from_raw(ptr, n) };
            view.assert_invariants();
            for i in 0..n as u64 {
                assert_eq!(Some(&(i * 2)), view.get(&i));
                assert_eq!(Some(i as usize), view.get_index_of(&i));
            }
            assert_eq!(None, view.get(&(n as u64)));
            // SAFETY: `u64` needs no drop, so freeing the block is enough.
            unsafe { alloc::dealloc(ptr.as_ptr(), layout) };
        }
    }
}
