/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! [`MiniMutInsertionMap`], the mutable map that iterates in insertion order.

use std::alloc;
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
#[cfg(feature = "pagable_dep")]
use pagable::PagableDeserialize;
#[cfg(feature = "pagable_dep")]
use pagable::PagableSerialize;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::MiniFixedInsertionMap;
use crate::arrays::Arrays;
use crate::arrays::DeallocGuard;
use crate::arrays::InitGuard;
use crate::arrays::alloc_block;
use crate::hashed::Hashed;
use crate::insertion_map::layout;
use crate::insertion_map::layout::Header;
use crate::insertion_map::layout::Offsets;
use crate::insertion_map::raw::Raw;
use crate::mut_insertion_map::iter::OwnedEntries;
use crate::short_hash::ShortHash;

pub(crate) mod iter;

pub use crate::mut_insertion_map::iter::IntoIter;
pub use crate::mut_insertion_map::iter::IntoIterHashed;
pub use crate::mut_insertion_map::iter::IntoKeys;
pub use crate::mut_insertion_map::iter::IntoValues;
pub use crate::mut_insertion_map::iter::Iter;
pub use crate::mut_insertion_map::iter::IterHashed;
pub use crate::mut_insertion_map::iter::IterMut;
pub use crate::mut_insertion_map::iter::IterMutUnchecked;
pub use crate::mut_insertion_map::iter::Keys;
pub use crate::mut_insertion_map::iter::Values;
pub use crate::mut_insertion_map::iter::ValuesMut;

/// What an empty map points at, so that reading the length never needs a branch. Never written:
/// every mutation first checks for a capacity of zero and allocates.
static EMPTY: Header = Header { len: 0, cap: 0 };

/// Smallest capacity a growing map allocates, matching `Vec` for small elements.
const MIN_NON_ZERO_CAP: usize = 4;

/// A mutable map that iterates in insertion order.
///
/// Similar to [`indexmap::IndexMap`](https://docs.rs/indexmap), with the differences that the
/// entries, their [short hashes](ShortHash) and the position index all live in one allocation
/// behind a one-word handle, and that no index is kept at all while the map is small enough that
/// scanning the hashes is faster.
///
/// `==` and `Hash` are order-sensitive, as the name promises and as they are for `Vec`: two maps
/// are equal when they hold the same entries in the same order. [`eq_unordered`](Self::eq_unordered)
/// answers the other question.
pub struct MiniMutInsertionMap<K, V> {
    /// The header of the allocation, or [`EMPTY`] when there is none.
    ptr: NonNull<Header>,
    _marker: PhantomData<(K, V)>,
}

// SAFETY: the map owns its keys and values outright, so it can cross threads exactly when they can.
unsafe impl<K: Send, V: Send> Send for MiniMutInsertionMap<K, V> {}
// SAFETY: `&MiniMutInsertionMap` hands out `&K`/`&V` and nothing else.
unsafe impl<K: Sync, V: Sync> Sync for MiniMutInsertionMap<K, V> {}

impl<K, V> Default for MiniMutInsertionMap<K, V> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Debug, V: Debug> Debug for MiniMutInsertionMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V> MiniMutInsertionMap<K, V> {
    /// Empty map. Does not allocate.
    #[inline]
    pub const fn new() -> Self {
        MiniMutInsertionMap {
            ptr: NonNull::from_ref(&EMPTY),
            _marker: PhantomData,
        }
    }

    /// Create an empty map with specified capacity.
    #[inline]
    pub fn with_capacity(n: usize) -> Self {
        let mut map = Self::new();
        if n > 0 {
            map.relocate(n);
        }
        map
    }

    #[inline]
    fn header(&self) -> &Header {
        // SAFETY: `ptr` is either `EMPTY` or the header of this map's allocation.
        unsafe { self.ptr.as_ref() }
    }

    #[inline]
    fn offsets(&self) -> Offsets<K, V> {
        layout::offsets::<K, V>(self.capacity(), true)
    }

    /// The allocation, or a view of nothing when there is none.
    #[inline]
    fn raw(&self) -> Raw<K, V> {
        let cap = self.capacity();
        if cap == 0 {
            Raw::dangling()
        } else {
            // SAFETY: a map with capacity owns an allocation laid out for it, with a header.
            unsafe { Raw::new(self.ptr.cast(), cap, true) }
        }
    }

    #[inline]
    fn arrays(&self) -> Arrays<K, V> {
        self.raw().arrays()
    }

    /// Set the length. The map must have an allocation.
    #[inline]
    unsafe fn set_len(&self, len: usize) {
        debug_assert!(self.capacity() > 0);
        // SAFETY: not `EMPTY`, per the caller, so this is this map's own header.
        unsafe { (*self.ptr.as_ptr()).len = len };
    }

    /// Move the entries to a fresh allocation with room for exactly `new_cap >= len` entries, or
    /// to no allocation at all when that is zero.
    fn relocate(&mut self, new_cap: usize) {
        let len = self.len();
        debug_assert!(new_cap >= len);
        let old = self.raw();
        if new_cap == 0 {
            self.ptr = NonNull::from_ref(&EMPTY);
        } else {
            let offsets = layout::checked_offsets::<K, V>(new_cap, true);
            let ptr = alloc_block(offsets.layout);
            // SAFETY: freshly allocated with this layout; the old arrays hold `len` entries, which
            // are moved rather than copied because the old block is freed without dropping them.
            unsafe {
                let new = Raw::new(ptr, new_cap, true);
                ptr::write(new.header(), Header { len, cap: new_cap });
                new.arrays().copy_from(old.arrays(), len);
                new.index_rebuild(len);
            }
            self.ptr = ptr.cast();
        }
        if old.cap() > 0 {
            // SAFETY: the old block was allocated with this layout and nothing refers to it.
            unsafe { alloc::dealloc(old.base().as_ptr(), old.layout()) };
        }
    }

    /// Make room for `additional` more entries.
    #[inline]
    fn grow_for(&mut self, additional: usize) {
        let required = self
            .len()
            .checked_add(additional)
            .expect("capacity overflow");
        let cap = self.capacity();
        if required > cap {
            self.grow_to(required.max(cap * 2).max(MIN_NON_ZERO_CAP));
        }
    }

    #[cold]
    fn grow_to(&mut self, new_cap: usize) {
        self.relocate(new_cap);
    }

    /// Verify that the map is internally consistent.
    #[cfg(test)]
    fn assert_invariants(&self)
    where
        K: Eq,
    {
        assert!(self.len() <= self.capacity());
        // SAFETY: `len` entries are initialized.
        unsafe { self.raw().assert_index_consistent(self.len()) };
        for (i, (k, _)) in self.iter_hashed().enumerate() {
            assert_eq!(Some(i), self.get_index_of_hashed(k));
        }
    }

    /// Give back the memory an index takes if the map has shrunk to where it needs none.
    ///
    /// An index is kept for as long as the map has the capacity that calls for one, however many
    /// entries are removed; this reallocates to the current length if that is below the threshold.
    pub fn maybe_drop_index(&mut self) {
        if layout::has_index(self.capacity()) && !layout::has_index(self.len()) {
            self.shrink_to_fit();
        }
    }

    /// Drop the spare capacity, and the index with it if the remaining entries do not need one.
    pub fn shrink_to_fit(&mut self) {
        if self.capacity() > self.len() {
            self.relocate(self.len());
        }
    }

    /// Key references iterator.
    #[inline]
    pub fn keys(&self) -> Keys<'_, K, V> {
        // SAFETY: `len` entries are initialized and borrowed for `'_`.
        Keys::new(unsafe { self.arrays().keys(self.len()) })
    }

    /// Value references iterator.
    #[inline]
    pub fn values(&self) -> Values<'_, K, V> {
        // SAFETY: as in `keys`.
        Values::new(unsafe { self.arrays().values(self.len()) })
    }

    /// Key owned iterator.
    #[inline]
    pub fn into_keys(self) -> IntoKeys<K, V> {
        IntoKeys::new(self.into_owned_entries())
    }

    /// Value owned iterator.
    #[inline]
    pub fn into_values(self) -> IntoValues<K, V> {
        IntoValues::new(self.into_owned_entries())
    }

    /// Mutable value references iterator.
    #[inline]
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        // SAFETY: as in `keys`, and `&mut self` makes the borrow exclusive.
        ValuesMut::new(unsafe { self.arrays().values_mut(self.len()) })
    }

    /// Entry references iterator.
    #[inline]
    pub fn iter(&self) -> Iter<'_, K, V> {
        let (arrays, len) = (self.arrays(), self.len());
        // SAFETY: as in `keys`.
        unsafe { Iter::new(arrays.keys(len), arrays.values(len)) }
    }

    /// Entry references with hashes iterator.
    #[inline]
    pub fn iter_hashed(&self) -> IterHashed<'_, K, V> {
        let (arrays, len) = (self.arrays(), self.len());
        // SAFETY: as in `keys`.
        unsafe { IterHashed::new(arrays.keys(len), arrays.values(len), arrays.hashes(len)) }
    }

    /// Entries with hashes iterator.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<K, V> {
        IntoIterHashed::new(self.into_owned_entries())
    }

    /// Mutable entry references iterator.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        let (arrays, len) = (self.arrays(), self.len());
        // SAFETY: as in `values_mut`; keys and values are disjoint regions.
        unsafe { IterMut::new(arrays.keys(len), arrays.values_mut(len)) }
    }

    /// Mutable entry references iterator, with mutable key references.
    ///
    /// This operation is memory safe, but otherwise no guarantees
    /// if keys are mutated inconsistently (hash or equality changes).
    #[inline]
    pub fn iter_mut_unchecked(&mut self) -> IterMutUnchecked<'_, K, V> {
        let (arrays, len) = (self.arrays(), self.len());
        // SAFETY: as in `iter_mut`.
        unsafe { IterMutUnchecked::new(arrays.keys_mut(len), arrays.values_mut(len)) }
    }

    /// Give up the allocation to an iterator that moves the entries out of it.
    fn into_owned_entries(self) -> OwnedEntries<K, V> {
        let len = self.len();
        let raw = self.raw();
        let block = if raw.cap() > 0 {
            Some((raw.base(), raw.layout()))
        } else {
            None
        };
        mem::forget(self);
        // SAFETY: the map held `len` entries in these arrays and is forgotten, so the iterator is
        // their only owner, and `block` is the allocation they are in.
        unsafe { OwnedEntries::new(raw.arrays(), len, block) }
    }

    /// Query the map by a prehashed key.
    #[inline(always)]
    pub fn get_hashed<Q>(&self, key: Hashed<&Q>) -> Option<&V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let raw = self.raw();
        let arrays = raw.arrays();
        let i = self.find_in(raw, arrays, key.hash(), move |k| key.key().equivalent(k))?;
        // SAFETY: `i` is the position of an entry.
        Some(unsafe { arrays.value(i) })
    }

    /// Same as `get_hashed`, byt takes key by value instead of by reference.
    /// Sometimes it generates slightly better code for small values.
    #[inline(always)]
    pub fn get_hashed_by_value<Q>(&self, key: Hashed<Q>) -> Option<&V>
    where
        Q: Equivalent<K>,
    {
        let raw = self.raw();
        let arrays = raw.arrays();
        let i = self.find_in(raw, arrays, key.hash(), move |k| key.key().equivalent(k))?;
        // SAFETY: `i` is the position of an entry.
        Some(unsafe { arrays.value(i) })
    }

    /// Query the map by a given key.
    #[inline(always)]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_hashed(Hashed::new(key))
    }

    /// Query the map by a given key, return an index of the entry
    /// along with the entry key and value.
    #[inline(always)]
    pub fn get_full<Q>(&self, key: &Q) -> Option<(usize, &K, &V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_full_hashed(Hashed::new(key))
    }

    /// Query the map by a given key, return an index of the entry
    /// along with the entry key and value.
    #[inline(always)]
    pub fn get_full_hashed<Q>(&self, key: Hashed<&Q>) -> Option<(usize, &K, &V)>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let raw = self.raw();
        let arrays = raw.arrays();
        let i = self.find_in(raw, arrays, key.hash(), move |k| key.key().equivalent(k))?;
        // SAFETY: `i` is the position of an entry.
        Some(unsafe { (i, arrays.key(i), arrays.value(i)) })
    }

    /// Position of the entry with this hash for which `eq` holds, in `raw`, which must be this
    /// map's allocation, with `arrays` its arrays. Callers that go on to touch the entry pass both
    /// on rather than deriving them a second time.
    ///
    /// This and the lookup entry points (`get*`, `contains*`, `insert*`, and their counterparts on
    /// the sets and the fixed map) are `#[inline(always)]` rather than `#[inline]`. The inlined
    /// lookup, header load plus closed-form offsets plus hash scan or group probe, sits just past
    /// LLVM's inlining threshold, and left out of line it costs a call with five register saves
    /// per lookup, about a quarter of a small map's lookup time (`mini_map_bench`). hashbrown's
    /// lookups inline everywhere for the same reason. The facade types in starlark_map reach
    /// these methods through `Deref`, so the attribute needs no repeating there.
    #[inline(always)]
    fn find_in(
        &self,
        raw: Raw<K, V>,
        arrays: Arrays<K, V>,
        hash: ShortHash,
        mut eq: impl FnMut(&K) -> bool,
    ) -> Option<usize> {
        // SAFETY: `len` entries are initialized, and `find` only hands out their positions.
        unsafe { raw.find(self.len(), hash.get(), move |i| eq(arrays.key(i))) }
    }

    #[inline(always)]
    pub(crate) fn get_index_of_hashed_raw(
        &self,
        hash: ShortHash,
        eq: impl FnMut(&K) -> bool,
    ) -> Option<usize> {
        let raw = self.raw();
        self.find_in(raw, raw.arrays(), hash, eq)
    }

    /// Find the index of the given hashed key.
    #[inline(always)]
    pub fn get_index_of_hashed<Q>(&self, key: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.get_index_of_hashed_raw(key.hash(), |k| key.key().equivalent(k))
    }

    /// Get the index of the entry given a hashed key.
    #[inline(always)]
    pub fn get_index_of_hashed_by_value<Q>(&self, key: Hashed<Q>) -> Option<usize>
    where
        Q: Equivalent<K>,
    {
        self.get_index_of_hashed_raw(key.hash(), |k| key.key().equivalent(k))
    }

    /// Find an entry by an index.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        if index >= self.len() {
            return None;
        }
        let arrays = self.arrays();
        // SAFETY: `index < len`.
        Some(unsafe { (arrays.key(index), arrays.value(index)) })
    }

    /// The an entry index by a given key.
    #[inline(always)]
    pub fn get_index_of<Q>(&self, key: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_index_of_hashed(Hashed::new(key))
    }

    /// Find a mutable value by a hashed key.
    #[inline(always)]
    pub fn get_mut_hashed<Q>(&mut self, key: Hashed<&Q>) -> Option<&mut V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        let raw = self.raw();
        let arrays = raw.arrays();
        let i = self.find_in(raw, arrays, key.hash(), move |k| key.key().equivalent(k))?;
        // SAFETY: `i` is the position of an entry, borrowed exclusively through `&mut self`.
        Some(unsafe { arrays.value_mut(i) })
    }

    /// Find the entry by a given key.
    #[inline(always)]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.get_mut_hashed(Hashed::new(key))
    }

    /// Find if an entry by a given prehashed key exists.
    #[inline(always)]
    pub fn contains_key_hashed<Q>(&self, key: Hashed<&Q>) -> bool
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.get_index_of_hashed(key).is_some()
    }

    /// Find if an entry by a given hashed key exists.
    #[inline]
    pub fn contains_key_hashed_by_value<Q>(&self, key: Hashed<Q>) -> bool
    where
        Q: Equivalent<K>,
    {
        self.get_index_of_hashed_by_value(key).is_some()
    }

    /// Find if an entry by a given key exists.
    #[inline(always)]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.contains_key_hashed(Hashed::new(key))
    }

    /// Reserve capacity for at least `additional` more elements to be inserted.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.grow_for(additional);
    }

    /// Current map capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.header().cap
    }

    /// Returns a reference to the first key-value pair.
    pub fn first(&self) -> Option<(&K, &V)> {
        self.iter().next()
    }

    /// Returns a reference to the last key-value pair.
    pub fn last(&self) -> Option<(&K, &V)> {
        self.iter().next_back()
    }

    /// Insert an entry into the map without checking for a duplicate key.
    #[inline]
    pub fn insert_hashed_unique_unchecked(&mut self, key: Hashed<K>, val: V) -> (&K, &mut V) {
        let len = self.len();
        if len == self.capacity() {
            self.grow_for(1);
        }
        let hash = key.hash().get();
        let raw = self.raw();
        let arrays = raw.arrays();
        // SAFETY: slot `len` is within capacity and unused; the index, if any, has room for one
        // more by construction of its size.
        unsafe {
            arrays.write(len, hash, key.into_key(), val);
            raw.index_insert(hash, len);
            self.set_len(len + 1);
            (arrays.key(len), arrays.value_mut(len))
        }
    }

    /// Insert a key-value pair into the map.
    #[inline(always)]
    pub fn insert_hashed(&mut self, key: Hashed<K>, val: V) -> Option<V>
    where
        K: Eq,
    {
        let raw = self.raw();
        let arrays = raw.arrays();
        match self.find_in(raw, arrays, key.hash(), |k| key.key().equivalent(k)) {
            None => {
                self.insert_hashed_unique_unchecked(key, val);
                None
            }
            // SAFETY: `i` is the position of an entry.
            Some(i) => Some(mem::replace(unsafe { arrays.value_mut(i) }, val)),
        }
    }

    /// Insert a key-value pair into the map.
    #[inline(always)]
    pub fn insert(&mut self, key: K, val: V) -> Option<V>
    where
        K: Hash + Eq,
    {
        self.insert_hashed(Hashed::new(key), val)
    }

    /// Insert a key-value pair into the map without checking for a duplicate key.
    #[inline]
    pub fn insert_unique_unchecked(&mut self, key: K, val: V) -> (&K, &mut V)
    where
        K: Hash,
    {
        self.insert_hashed_unique_unchecked(Hashed::new(key), val)
    }

    /// Take entry `i` out, shifting the ones after it down.
    fn remove_at(&mut self, i: usize) -> (Hashed<K>, V) {
        let len = self.len();
        debug_assert!(i < len);
        let raw = self.raw();
        let arrays = raw.arrays();
        // SAFETY: `i < len`. The index is updated while the hashes it refers to are still in
        // place, then the entries above `i` move down by one, which is what the renumbering
        // anticipates.
        unsafe {
            let (hash, key, value) = arrays.read(i);
            raw.index_remove(len, hash, i);
            if i + 1 < len {
                raw.index_map_positions(|p| if p > i { p - 1 } else { p });
                arrays.shift(i + 1, i, len - i - 1);
            }
            self.set_len(len - 1);
            (
                Hashed::new_unchecked(ShortHash::new_unchecked(hash), key),
                value,
            )
        }
    }

    /// Remove the entry for the key.
    ///
    /// Time complexity of this operation is *O(N)* where *N* is the number of entries in the map.
    pub fn shift_remove_hashed<Q>(&mut self, key: Hashed<&Q>) -> Option<V>
    where
        Q: ?Sized + Equivalent<K>,
    {
        self.shift_remove_hashed_entry(key).map(|(_k, v)| v)
    }

    /// Remove the entry for the key.
    ///
    /// Time complexity of this operation is *O(N)* where *N* is the number of entries in the map.
    pub fn shift_remove_hashed_entry<Q>(&mut self, key: Hashed<&Q>) -> Option<(K, V)>
    where
        Q: ?Sized + Equivalent<K>,
    {
        let i = self.get_index_of_hashed(key)?;
        let (key, value) = self.remove_at(i);
        Some((key.into_key(), value))
    }

    /// Remove the entry by index. This is *O(N)* operation.
    pub fn shift_remove_index_hashed(&mut self, i: usize) -> Option<(Hashed<K>, V)> {
        if i >= self.len() {
            return None;
        }
        Some(self.remove_at(i))
    }

    /// Remove the entry by index. This is *O(N)* operation.
    pub fn shift_remove_index(&mut self, i: usize) -> Option<(K, V)> {
        let (key, value) = self.shift_remove_index_hashed(i)?;
        Some((key.into_key(), value))
    }

    /// Remove the entry for the key.
    ///
    /// Time complexity of this operation is *O(N)* where *N* is the number of entries in the map.
    pub fn shift_remove<Q>(&mut self, key: &Q) -> Option<V>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.shift_remove_hashed(Hashed::new(key))
    }

    /// Remove the entry for the key.
    ///
    /// Time complexity of this operation is *O(N)* where *N* is the number of entries in the map.
    pub fn shift_remove_entry<Q>(&mut self, key: &Q) -> Option<(K, V)>
    where
        Q: ?Sized + Hash + Equivalent<K>,
    {
        self.shift_remove_hashed_entry(Hashed::new(key))
    }

    /// Get the entry (occupied or not) for the key.
    #[inline]
    pub fn entry_hashed(&mut self, key: Hashed<K>) -> Entry<'_, K, V>
    where
        K: Eq,
    {
        let raw = self.raw();
        let arrays = raw.arrays();
        match self.find_in(raw, arrays, key.hash(), |k| key.key().equivalent(k)) {
            Some(i) => {
                // SAFETY: `i` is the position of an entry, borrowed exclusively through
                // `&mut self`, and the key and value are disjoint.
                let (key, value) = unsafe { (arrays.key(i), arrays.value_mut(i)) };
                Entry::Occupied(OccupiedEntry { key, value })
            }
            None => Entry::Vacant(VacantEntry { key, map: self }),
        }
    }

    /// Remove the last element.
    pub fn pop(&mut self) -> Option<(K, V)> {
        let len = self.len();
        if len == 0 {
            return None;
        }
        let (key, value) = self.remove_at(len - 1);
        Some((key.into_key(), value))
    }

    /// Get the entry (occupied or not) for the key.
    #[inline]
    pub fn entry(&mut self, key: K) -> Entry<'_, K, V>
    where
        K: Eq + Hash,
    {
        self.entry_hashed(Hashed::new(key))
    }

    /// Is the map empty?
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the number of elements in the map.
    #[inline]
    pub fn len(&self) -> usize {
        self.header().len
    }

    /// Remove all elements from the map.
    ///
    /// Retain the capacity.
    #[inline]
    pub fn clear(&mut self) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let raw = self.raw();
        // SAFETY: `len` entries are initialized. The length and index go first so that an entry
        // whose drop panics leaves a consistent, if leaky, empty map.
        unsafe {
            self.set_len(0);
            raw.index_clear();
            raw.arrays().drop_entries(len);
        }
    }

    /// Sort the entries by key.
    pub fn sort_keys(&mut self)
    where
        K: Ord,
    {
        let len = self.len();
        let raw = self.raw();
        let arrays = raw.arrays();
        // SAFETY: `len` entries are initialized, and `keys` is not used once anything moves.
        let keys = unsafe { arrays.keys(len) };
        // Checked first so that the index is rebuilt only when something moved.
        if keys.is_sorted() {
            return;
        }
        // The order is worked out on positions and applied afterwards: a key comparison that
        // panics then finds nothing moved and the index still right, and the entries move once
        // each instead of once per comparison across three arrays.
        const STACK_POSITIONS: usize = 32;
        let mut stack = [0usize; STACK_POSITIONS];
        let mut heap: Vec<usize>;
        let order: &mut [usize] = if len <= STACK_POSITIONS {
            &mut stack[..len]
        } else {
            heap = vec![0; len];
            &mut heap[..]
        };
        for (i, slot) in order.iter_mut().enumerate() {
            *slot = i;
        }
        // Keys are distinct, so the sort being unstable changes nothing.
        order.sort_unstable_by(|&a, &b| keys[a].cmp(&keys[b]));
        // SAFETY: a permutation of `0..len`, and `len` entries are initialized; `permute` cannot
        // panic, and the rebuild after it sees all of them in their new places.
        unsafe {
            arrays.permute(order);
            raw.index_rebuild(len);
        }
    }

    /// Equal if the keys and values are equal in the iteration order.
    pub fn eq_ordered(&self, other: &Self) -> bool
    where
        K: PartialEq,
        V: PartialEq,
    {
        let (a, b) = (self.arrays(), other.arrays());
        let (n, m) = (self.len(), other.len());
        // SAFETY: each map has its `len` entries initialized.
        unsafe {
            // The stored hashes settle almost every inequality without touching a key.
            n == m
                && a.hashes(n) == b.hashes(m)
                && a.keys(n) == b.keys(m)
                && a.values(n) == b.values(m)
        }
    }

    /// Equal if the same keys map to equal values, whatever the iteration order.
    pub fn eq_unordered(&self, other: &Self) -> bool
    where
        K: Eq,
        V: PartialEq,
    {
        self.len() == other.len()
            && self
                .iter_hashed()
                .all(|(k, v)| other.get_hashed(k) == Some(v))
    }

    /// Hash entries in the iteration order.
    ///
    /// Note, keys are not hashed, but previously computed hashes are hashed instead.
    pub fn hash_ordered<H: Hasher>(&self, state: &mut H)
    where
        K: Hash,
        V: Hash,
    {
        for e in self.iter_hashed() {
            e.hash(state);
        }
    }

    /// Reverse the iteration order of the map.
    pub fn reverse(&mut self) {
        let len = self.len();
        let raw = self.raw();
        // SAFETY: `len` entries are initialized; reversing moves no entry to a different hash.
        unsafe {
            raw.arrays().reverse(len);
            raw.index_map_positions(|p| len - 1 - p);
        }
    }

    /// Retains only the elements specified by the predicate.
    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&K, &mut V) -> bool,
    {
        let original_len = self.len();
        if original_len == 0 {
            return;
        }
        let raw = self.raw();
        let arrays = raw.arrays();

        // The survivors are packed at the front, `kept` of them; the `deleted` slots after them
        // are dead; everything from `kept + deleted` on is still untouched. If `f` panics, the
        // untouched tail is packed down behind the survivors, which is also what finishes a
        // normal run.
        struct Guard<K, V> {
            raw: Raw<K, V>,
            original_len: usize,
            kept: usize,
            deleted: usize,
        }
        impl<K, V> Drop for Guard<K, V> {
            fn drop(&mut self) {
                if self.deleted == 0 {
                    return;
                }
                let scanned = self.kept + self.deleted;
                let len = self.original_len - self.deleted;
                // SAFETY: the tail `scanned..original_len` is initialized and nothing is at
                // `kept..scanned` any more; afterwards `len` entries are initialized.
                unsafe {
                    self.raw
                        .arrays()
                        .shift(scanned, self.kept, self.original_len - scanned);
                    (*self.raw.header()).len = len;
                    self.raw.index_rebuild(len);
                }
            }
        }
        let mut guard = Guard {
            raw,
            original_len,
            kept: 0,
            deleted: 0,
        };
        for i in 0..original_len {
            // SAFETY: entry `i` is initialized and untouched so far.
            if unsafe { f(arrays.key(i), arrays.value_mut(i)) } {
                if guard.deleted > 0 {
                    // SAFETY: slot `kept` is dead, so the move leaves `i` dead instead.
                    unsafe { arrays.shift(i, guard.kept, 1) };
                }
                guard.kept += 1;
            } else {
                // Counted as dead before being dropped, so that a panicking drop does not get
                // it dropped a second time by the guard.
                guard.deleted += 1;
                // SAFETY: entry `i` is initialized and no longer part of the map.
                unsafe { arrays.offset(i).drop_entries(1) };
            }
        }
    }

    /// Convert to a [`MiniFixedInsertionMap`], keeping the entries, their order and their hashes.
    ///
    /// A map past the index threshold hands its allocation over after shrinking it to fit;
    /// smaller maps are copied into one without a header.
    pub fn freeze(mut self) -> MiniFixedInsertionMap<K, V> {
        let len = self.len();
        if len == 0 {
            return MiniFixedInsertionMap::new();
        }
        if layout::has_index(len) {
            self.shrink_to_fit();
            let ptr = self.ptr.cast::<u8>();
            mem::forget(self);
            // SAFETY: with the capacity equal to the length, the allocation has exactly the
            // layout of a large fixed map of `len` entries.
            unsafe { MiniFixedInsertionMap::from_raw_parts(ptr, len) }
        } else {
            let offsets = layout::checked_offsets::<K, V>(len, false);
            let ptr = alloc_block(offsets.layout);
            let old = self.raw();
            // SAFETY: freshly allocated with the small fixed layout; the entries are moved, and
            // the old block is freed without dropping them.
            unsafe {
                Raw::new(ptr, len, false)
                    .arrays()
                    .copy_from(old.arrays(), len);
                alloc::dealloc(old.base().as_ptr(), old.layout());
            }
            mem::forget(self);
            // SAFETY: just laid out and filled as a small fixed map of `len` entries.
            unsafe { MiniFixedInsertionMap::from_raw_parts(ptr, len) }
        }
    }
}

impl<K, V> MiniMutInsertionMap<K, V> {
    fn drop_impl(&mut self) {
        let raw = self.raw();
        if raw.cap() == 0 {
            return;
        }
        let _guard = DeallocGuard {
            ptr: raw.base(),
            layout: raw.layout(),
        };
        // SAFETY: `len` entries are initialized and owned by this map.
        unsafe { raw.arrays().drop_entries(self.len()) };
    }
}

// SAFETY: `may_dangle` lets a map holding borrowed data be dropped after that data, as `Vec`
// allows, because dropping touches the entries only to drop them; see the Rustonomicon on drop
// check. It is nightly-only, so without `rust_nightly` a plain `Drop` impl applies with the stricter
// rules that come with it.
#[cfg(rust_nightly)]
unsafe impl<#[may_dangle] K, #[may_dangle] V> Drop for MiniMutInsertionMap<K, V> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

#[cfg(not(rust_nightly))]
impl<K, V> Drop for MiniMutInsertionMap<K, V> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

impl<K: Clone, V: Clone> Clone for MiniMutInsertionMap<K, V> {
    /// The clone has no spare capacity.
    fn clone(&self) -> Self {
        let len = self.len();
        if len == 0 {
            return Self::new();
        }
        let offsets = layout::checked_offsets::<K, V>(len, true);
        let ptr = alloc_block(offsets.layout);
        let dealloc = DeallocGuard {
            ptr,
            layout: offsets.layout,
        };
        // SAFETY: freshly allocated with this layout.
        let new = unsafe { Raw::new(ptr, len, true) };
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
        unsafe {
            ptr::write(new.header(), Header { len, cap: len });
            new.index_rebuild(len);
        }
        mem::forget(guard);
        mem::forget(dealloc);
        MiniMutInsertionMap {
            ptr: ptr.cast(),
            _marker: PhantomData,
        }
    }
}

/// Reference to the actual entry in the map.
pub struct OccupiedEntry<'a, K, V> {
    /// Pointer to the key in the map.
    key: &'a K,
    /// Pointer to the value in the map.
    value: &'a mut V,
}

/// Reference to a vacant entry in the map.
///
/// This can be used to insert an entry into the map.
pub struct VacantEntry<'a, K, V> {
    key: Hashed<K>,
    map: &'a mut MiniMutInsertionMap<K, V>,
}

/// Occupied or vacant entry.
pub enum Entry<'a, K, V> {
    /// Occupied entry.
    Occupied(OccupiedEntry<'a, K, V>),
    /// No entry for given key.
    Vacant(VacantEntry<'a, K, V>),
}

impl<'a, K, V> OccupiedEntry<'a, K, V> {
    /// Key for this entry.
    #[inline]
    pub fn key(&self) -> &K {
        self.key
    }

    /// Value for this entry.
    #[inline]
    pub fn get(&self) -> &V {
        self.value
    }

    /// Mutable reference to the value in the entry.
    #[inline]
    pub fn get_mut(&mut self) -> &mut V {
        self.value
    }

    /// Get a reference to the value in the entry with map lifetime.
    #[inline]
    pub fn into_mut(self) -> &'a mut V {
        self.value
    }

    #[inline]
    pub(crate) fn into_mut_entry(self) -> (&'a K, &'a mut V) {
        (self.key, self.value)
    }

    /// Get access to both the key and the value in the entry
    #[inline]
    pub fn as_key_and_mut_value(&mut self) -> (&K, &mut V) {
        (self.key, self.value)
    }
}

impl<'a, K, V> VacantEntry<'a, K, V>
where
    K: Eq,
{
    /// Key for this entry.
    #[inline]
    pub fn key(&self) -> &K {
        self.key.key()
    }

    /// Insert the value into the entry.
    #[inline(always)]
    pub fn insert(self, value: V) -> &'a mut V {
        self.insert_entry(value).1
    }

    #[inline]
    pub(crate) fn insert_entry(self, value: V) -> (&'a K, &'a mut V) {
        self.map.insert_hashed_unique_unchecked(self.key, value)
    }
}

impl<'a, K, V> Entry<'a, K, V>
where
    K: Eq,
{
    /// Key for this entry.
    #[inline]
    pub fn key(&self) -> &K {
        match self {
            Entry::Occupied(e) => e.key(),
            Entry::Vacant(e) => e.key(),
        }
    }

    /// Insert if vacant.
    #[inline]
    pub fn or_insert(self, default: V) -> &'a mut V {
        self.or_insert_with(|| default)
    }

    /// Insert if vacant.
    #[inline]
    pub fn or_insert_with(self, default: impl FnOnce() -> V) -> &'a mut V {
        self.or_insert_entry_with(default).1
    }

    /// Insert if vacant.
    #[inline]
    pub fn or_default(self) -> &'a mut V
    where
        V: Default,
    {
        #[allow(clippy::unwrap_or_default)] // defining or_default
        self.or_insert_with(V::default)
    }

    #[inline]
    pub(crate) fn or_insert_entry_with(self, default: impl FnOnce() -> V) -> (&'a K, &'a mut V) {
        match self {
            Entry::Occupied(e) => e.into_mut_entry(),
            Entry::Vacant(e) => e.insert_entry(default()),
        }
    }

    /// Modify if present
    #[inline]
    pub fn and_modify<F>(self, f: F) -> Self
    where
        F: FnOnce(&mut V),
    {
        match self {
            Entry::Occupied(mut entry) => {
                f(entry.get_mut());
                Entry::Occupied(entry)
            }
            Entry::Vacant(entry) => Entry::Vacant(entry),
        }
    }
}

impl<K, V> FromIterator<(K, V)> for MiniMutInsertionMap<K, V>
where
    K: Hash + Eq,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let mut mp = Self::with_capacity(iter.size_hint().0);
        for (k, v) in iter {
            mp.insert(k, v);
        }
        mp
    }
}

impl<K, V> FromIterator<(Hashed<K>, V)> for MiniMutInsertionMap<K, V>
where
    K: Eq,
{
    fn from_iter<I: IntoIterator<Item = (Hashed<K>, V)>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let mut mp = Self::with_capacity(iter.size_hint().0);
        for (k, v) in iter {
            mp.insert_hashed(k, v);
        }
        mp
    }
}

impl<K, V> IntoIterator for MiniMutInsertionMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter::new(self.into_owned_entries())
    }
}

impl<'a, K, V> IntoIterator for &'a MiniMutInsertionMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut MiniMutInsertionMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Order-sensitive: the same entries in the same order, as for `Vec`. See
/// [`eq_unordered`](MiniMutInsertionMap::eq_unordered) for the other question.
impl<K: PartialEq, V: PartialEq> PartialEq for MiniMutInsertionMap<K, V> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.eq_ordered(other)
    }
}

impl<K: Eq, V: Eq> Eq for MiniMutInsertionMap<K, V> {}

/// Order-sensitive, to agree with `==`; the same as [`hash_ordered`](MiniMutInsertionMap::hash_ordered).
impl<K: Hash, V: Hash> Hash for MiniMutInsertionMap<K, V> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.hash_ordered(state)
    }
}

impl<K: StrongHash, V: StrongHash> StrongHash for MiniMutInsertionMap<K, V> {
    /// Hashes the entries in the iteration order, which the caller owns.
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.len().strong_hash(state);
        for (k, v) in self.iter() {
            k.strong_hash(state);
            v.strong_hash(state);
        }
    }
}

impl<K: Hash + Eq, V, const N: usize> From<[(K, V); N]> for MiniMutInsertionMap<K, V> {
    /// A repeated key keeps its first position and its last value, as a sequence of inserts would.
    fn from(entries: [(K, V); N]) -> Self {
        Self::from_iter(entries)
    }
}

impl<K, V> Extend<(K, V)> for MiniMutInsertionMap<K, V>
where
    K: Hash + Eq,
{
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<K, V> From<MiniMutInsertionMap<K, V>> for MiniFixedInsertionMap<K, V> {
    #[inline]
    fn from(map: MiniMutInsertionMap<K, V>) -> Self {
        map.freeze()
    }
}

impl<K: Allocative, V: Allocative> Allocative for MiniMutInsertionMap<K, V> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        let (len, cap) = (self.len(), self.capacity());
        if cap > 0 {
            let offsets = self.offsets();
            let arrays = self.arrays();
            let mut visitor =
                visitor.enter_unique(allocative::Key::new("ptr"), size_of::<*const u8>());
            visitor.visit_simple(allocative::Key::new("header"), size_of::<Header>());
            visitor.visit_simple(allocative::Key::new("hashes"), size_of::<u32>() * cap);
            {
                let mut visitor = visitor.enter(allocative::Key::new("keys"), size_of::<K>() * cap);
                // SAFETY: `len` entries are initialized.
                visitor.visit_slice::<K>(unsafe { arrays.keys(len) });
                visitor.exit();
            }
            {
                let mut visitor =
                    visitor.enter(allocative::Key::new("values"), size_of::<V>() * cap);
                // SAFETY: `len` entries are initialized.
                visitor.visit_slice::<V>(unsafe { arrays.values(len) });
                visitor.exit();
            }
            let index = offsets.layout.size() - offsets.index;
            if index > 0 {
                visitor.visit_simple(allocative::Key::new("index"), index);
            }
            // Whatever `Layout::extend` inserted between the regions, so the reported size is the
            // size of the real allocation.
            let accounted = size_of::<Header>()
                + (size_of::<u32>() + size_of::<K>() + size_of::<V>()) * cap
                + index;
            let padding = offsets.layout.size() - accounted;
            if padding > 0 {
                visitor.visit_simple(allocative::Key::new("padding"), padding);
            }
            visitor.exit();
        }
        visitor.exit();
    }
}

/// Create a [`MiniMutInsertionMap`](MiniMutInsertionMap) from a list of key-value pairs.
///
/// ## Example
///
/// ```
/// use mini_map::mini_mut_insertion_map;
///
/// let map = mini_mut_insertion_map! {
///     "a" => 1,
///     "b" => 2,
/// };
/// assert_eq!(map.get("a"), Some(&1));
/// assert_eq!(map.get("b"), Some(&2));
/// assert_eq!(map.get("c"), None);
/// ```
#[macro_export]
macro_rules! mini_mut_insertion_map {
    (@single $($x:tt)*) => (());
    (@count $($rest:expr),*) => (<[()]>::len(&[$(mini_mut_insertion_map!(@single $rest)),*]));

    ($($key:expr => $value:expr,)+) => { mini_mut_insertion_map!($($key => $value),+) };
    ($($key:expr => $value:expr),*) => {
        {
            let cap = mini_mut_insertion_map!(@count $($key),*);
            #[allow(unused_mut)]
            let mut map = $crate::mut_insertion_map::MiniMutInsertionMap::with_capacity(cap);
            $(
                map.insert($key, $value);
            )*
            map
        }
    };
}

#[cfg(feature = "pagable_dep")]
impl<K: pagable::Pagable, V: pagable::Pagable> PagableSerialize for MiniMutInsertionMap<K, V> {
    /// Writes the length, then each entry's key, value and stored hash in iteration order.
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
    for MiniMutInsertionMap<K, V>
{
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        let len = usize::deserialize(deserializer.serde())?;
        let mut map = Self::with_capacity(len);
        for _ in 0..len {
            let key = K::pagable_deserialize(deserializer)?;
            let value = V::pagable_deserialize(deserializer)?;
            let hash = ShortHash::pagable_deserialize(deserializer)?;
            map.insert_hashed_unique_unchecked(Hashed::new_unchecked(hash, key), value);
        }
        Ok(map)
    }
}

impl<K: Serialize, V: Serialize> Serialize for MiniMutInsertionMap<K, V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_map(self.iter())
    }
}

impl<'de, K, V> Deserialize<'de> for MiniMutInsertionMap<K, V>
where
    K: Deserialize<'de> + Hash + Eq,
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct MapVisitor<K, V> {
            marker: PhantomData<MiniMutInsertionMap<K, V>>,
        }

        impl<'de, K, V> serde::de::Visitor<'de> for MapVisitor<K, V>
        where
            K: Deserialize<'de> + Hash + Eq,
            V: Deserialize<'de>,
        {
            type Value = MiniMutInsertionMap<K, V>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a map")
            }

            #[inline]
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut values = MiniMutInsertionMap::with_capacity(map.size_hint().unwrap_or(0));
                while let Some((key, value)) = map.next_entry()? {
                    values.insert(key, value);
                }
                Ok(values)
            }
        }

        let visitor = MapVisitor {
            marker: PhantomData,
        };
        deserializer.deserialize_map(visitor)
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::cmp::Ordering;
    use std::panic::AssertUnwindSafe;
    use std::panic::catch_unwind;
    use std::rc::Rc;

    use super::*;

    #[test]
    fn empty_map() {
        let m = MiniMutInsertionMap::<i8, &str>::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert_eq!(m.iter().next(), None);
        assert_eq!(0, m.capacity());
    }

    #[test]
    #[allow(clippy::map_identity)]
    fn few_entries() {
        let entries1 = [(0, 'a'), (1, 'b')];
        let m1 = entries1
            .iter()
            .copied()
            .collect::<MiniMutInsertionMap<_, _>>();

        let entries2 = [(1, 'b'), (0, 'a')];
        let m2 = entries2
            .iter()
            .copied()
            .collect::<MiniMutInsertionMap<_, _>>();
        assert!(!m1.is_empty());
        assert_eq!(m1.len(), 2);
        assert!(!m2.is_empty());
        assert_eq!(m2.len(), 2);

        assert!(m1.iter().eq(entries1.iter().map(|(k, v)| (k, v))));
        assert!(m2.iter().eq(entries2.iter().map(|(k, v)| (k, v))));
        assert!(!m1.iter().eq(m2.iter()));
        assert!(m1.eq_unordered(&m1));
        assert!(m2.eq_unordered(&m2));
        assert!(m1.eq_unordered(&m2));

        assert_eq!(m1.get(&0), Some(&'a'));
        assert_eq!(m1.get(&3), None);
        assert_eq!(m2.get(&1), Some(&'b'));
        assert_eq!(m2.get(&3), None);

        assert_eq!(m1.get_index(0), Some((&0, &'a')));
        assert_eq!(m1.get_index(1), Some((&1, &'b')));
        assert_eq!(m1.get_index(2), None);

        assert!(!m1.eq_unordered(&mini_mut_insertion_map! { 0 => 'a', 1 => 'c' }));

        let iter = m1.iter();
        let (values1, values2): (Vec<_>, Vec<_>) = (iter.clone().collect(), iter.collect());
        assert_eq!(values1, values2);
    }

    #[test]
    fn many_entries() {
        let numbers = 0..26;
        let letters = 'a'..='z';

        let entries1 = numbers.zip(letters);
        let m1 = entries1.clone().collect::<MiniMutInsertionMap<_, _>>();

        let numbers = (0..26).rev();
        let letters = ('a'..='z').rev();
        let entries2 = numbers.zip(letters);
        let m2 = entries2.clone().collect::<MiniMutInsertionMap<_, _>>();
        assert!(!m1.is_empty());
        assert_eq!(m1.len(), 26);
        assert!(!m2.is_empty());
        assert_eq!(m2.len(), 26);

        assert!(m1.clone().into_iter().eq(entries1));
        assert!(m2.clone().into_iter().eq(entries2));
        assert!(!m1.iter().eq(m2.iter()));
        assert!(m1.eq_unordered(&m1));
        assert!(m2.eq_unordered(&m2));
        assert!(m1.eq_unordered(&m2));

        assert_eq!(m1.get(&1), Some(&'b'));
        assert_eq!(m1.get(&30), None);
        assert_eq!(m2.get(&0), Some(&'a'));
        assert_eq!(m2.get(&30), None);
        assert_eq!(m2.get_full(&0), Some((25, &0, &'a')));
        assert_eq!(m2.get_full(&25), Some((0, &25, &'z')));
        assert_eq!(m2.get_full(&29), None);

        let not_m1 = {
            let mut m = m1.clone();
            m.shift_remove(&1);
            m
        };
        assert!(!m1.eq_unordered(&not_m1));

        let iter = m1.iter();
        let (values1, values2): (Vec<_>, Vec<_>) = (iter.clone().collect(), iter.collect());
        assert_eq!(values1, values2);
    }

    #[test]
    fn test_smallmap_macro() {
        let map = mini_mut_insertion_map![1 => "a", 3 => "b"];
        let mut i = map.into_iter();
        assert_eq!(i.next(), Some((1, "a")));
        assert_eq!(i.next(), Some((3, "b")));
        assert_eq!(i.next(), None);
    }

    #[test]
    fn test_clone() {
        let map = mini_mut_insertion_map![1 => "a", 3 => "b"];
        let iter = map.iter();
        let values1: Vec<_> = iter.clone().collect();
        let values2: Vec<_> = iter.collect();
        assert_eq!(vec![(&1, &"a"), (&3, &"b")], values1);
        assert_eq!(values1, values2);

        let iter = map.keys();
        let values1: Vec<_> = iter.clone().collect();
        let values2: Vec<_> = iter.collect();
        assert_eq!(vec![&1, &3], values1);
        assert_eq!(values1, values2);

        let iter = map.values();
        let values1: Vec<_> = iter.clone().collect();
        let values2: Vec<_> = iter.collect();
        assert_eq!(vec![&"a", &"b"], values1);
        assert_eq!(values1, values2);
    }

    #[test]
    fn test_duplicate_hashes() {
        // A type which always gives hash collisions
        #[derive(PartialEq, Eq, Debug)]
        struct K(i32);
        #[allow(clippy::derived_hash_with_manual_eq)]
        impl Hash for K {
            fn hash<H: Hasher>(&self, _state: &mut H) {}
        }

        let mut map = mini_mut_insertion_map![K(1) => "test", K(3) => "more"];
        assert_eq!(map.get(&K(1)), Some(&"test"));
        assert_eq!(map.get(&K(2)), None);
        assert_eq!(map.get(&K(3)), Some(&"more"));

        assert_eq!(map.insert(K(2), "magic"), None);
        assert_eq!(map.get(&K(2)), Some(&"magic"));

        assert_eq!(map.shift_remove(&K(1)), Some("test"));
        assert_eq!(map.get(&K(1)), None);
        assert_eq!(map.keys().collect::<Vec<_>>(), vec![&K(3), &K(2)]);

        // Past the index threshold too, where every entry lands in one probe run.
        for i in 10..200 {
            map.insert(K(i), "x");
        }
        map.assert_invariants();
        for i in 10..200 {
            assert_eq!(Some(&"x"), map.get(&K(i)), "{i}");
        }
        assert_eq!(None, map.get(&K(5)));
    }

    #[test]
    fn test_smallmap_debug() {
        let s = format!("{:?}", mini_mut_insertion_map![1 => "test", 2 => "more"]);
        assert_eq!(s, "{1: \"test\", 2: \"more\"}")
    }

    #[test]
    fn entry() {
        let mut map = MiniMutInsertionMap::new();
        for i in 0..100 {
            match map.entry(i) {
                Entry::Vacant(e) => {
                    e.insert(i * 2);
                }
                Entry::Occupied(..) => panic!(),
            }
            match map.entry(i) {
                Entry::Occupied(..) => {}
                Entry::Vacant(..) => panic!(),
            }
        }
    }

    #[test]
    fn test_pop_small() {
        let mut map = MiniMutInsertionMap::new();
        for i in 0..=5 {
            map.insert(i, i * 10);
        }
        for i in (0..=5).rev() {
            assert_eq!((i, i * 10), map.pop().unwrap());
            map.assert_invariants();
        }
        assert!(map.is_empty());
        assert_eq!(None, map.pop());
    }

    #[test]
    fn test_pop_large() {
        let mut map = MiniMutInsertionMap::new();
        for i in 0..=500 {
            map.insert(i, i * 10);
        }
        for i in (0..=500).rev() {
            assert_eq!((i, i * 10), map.pop().unwrap());
            if i % 100 == 0 {
                map.assert_invariants();
            }
        }
        assert!(map.is_empty());
    }

    #[test]
    fn test_first() {
        let mut map = MiniMutInsertionMap::new();
        map.insert(1, 10);
        assert_eq!(map.first(), Some((&1, &10)));
        map.insert(2, 20);
        assert_eq!(map.first(), Some((&1, &10)));
        map.shift_remove(&1);
        assert_eq!(map.first(), Some((&2, &20)));
    }

    #[test]
    fn test_last() {
        let mut map = MiniMutInsertionMap::new();
        map.insert(1, 10);
        assert_eq!(map.last(), Some((&1, &10)));
        map.insert(2, 20);
        assert_eq!(map.last(), Some((&2, &20)));
        map.insert(1, 100);
        assert_eq!(map.last(), Some((&2, &20)));
    }

    #[test]
    fn test_sort_keys_no_index() {
        let mut map = MiniMutInsertionMap::new();
        map.insert(2, 20);
        map.insert(1, 10);
        map.insert(3, 30);
        map.sort_keys();
        assert_eq!(
            vec![(&1, &10), (&2, &20), (&3, &30)],
            map.iter().collect::<Vec<_>>()
        );
        assert_eq!(&10, map.get(&1).unwrap());
        assert_eq!(&20, map.get(&2).unwrap());
        assert_eq!(&30, map.get(&3).unwrap());
    }

    #[test]
    fn test_sort_keys_with_index() {
        let mut map = MiniMutInsertionMap::new();
        for i in (1..=100).rev() {
            map.insert(i, i * 10);
        }
        map.sort_keys();
        assert_eq!(
            (1..=100).map(|i| (i, i * 10)).collect::<Vec<_>>(),
            map.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
        );
        for i in 1..=100 {
            assert_eq!(i * 10, *map.get(&i).unwrap());
        }
        map.assert_invariants();
    }

    #[test]
    fn test_sort_keys_updates_index_on_panic() {
        #[derive(Hash, PartialEq, Eq, Debug)]
        struct Key(u32);

        impl PartialOrd for Key {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        impl Ord for Key {
            fn cmp(&self, other: &Self) -> Ordering {
                if self.0 < 10 && other.0 < 10 {
                    panic!("panic in Ord::cmp")
                }
                self.0.cmp(&other.0)
            }
        }

        let mut map = MiniMutInsertionMap::new();
        for i in (1..=100).rev() {
            map.insert(Key(i), i * 10);
        }
        catch_unwind(AssertUnwindSafe(|| map.sort_keys())).unwrap_err();
        // If index is not updated on panic, the following assertion will fail.
        map.assert_invariants();
    }

    #[test]
    fn test_eq_ordered() {
        let m0 = MiniMutInsertionMap::from_iter([(1, 2), (3, 4)]);
        let m1 = MiniMutInsertionMap::from_iter([(1, 2), (3, 4)]);
        let m2 = MiniMutInsertionMap::from_iter([(3, 4), (1, 2)]);
        let m3 = MiniMutInsertionMap::from_iter([(3, 4)]);
        assert!(m0.eq_ordered(&m0));
        assert!(m0.eq_ordered(&m1));
        assert!(!m0.eq_ordered(&m2));
        assert!(!m0.eq_ordered(&m3));
    }

    /// `==` and `Hash` are the ordered comparisons, and spare capacity does not take part.
    #[test]
    fn eq_and_hash_are_order_sensitive() {
        fn hash_of(map: &MiniMutInsertionMap<u32, u32>) -> u64 {
            let mut hasher = crate::FastHasher::new();
            map.hash(&mut hasher);
            hasher.finish()
        }

        let a = MiniMutInsertionMap::from_iter([(1, 2), (3, 4)]);
        let mut b = MiniMutInsertionMap::with_capacity(100);
        b.extend([(1, 2), (3, 4)]);
        let c = MiniMutInsertionMap::from_iter([(3, 4), (1, 2)]);
        assert_eq!(a, b);
        assert_eq!(hash_of(&a), hash_of(&b));
        assert_ne!(a, c);
        assert!(a.eq_unordered(&c));
    }

    #[test]
    fn test_shift_remove() {
        // Large enough so the index is used.
        let mut m = (0..100)
            .map(|i| (i, i * 10))
            .collect::<MiniMutInsertionMap<_, _>>();
        assert_eq!(Some((1, 10)), m.shift_remove_entry(&1));
        assert_eq!(Some(&30), m.get(&3));
        m.assert_invariants();
    }

    #[test]
    fn test_shift_remove_last() {
        // Large enough so the index is used.
        let mut m = (0..100)
            .map(|i| (i, i * 10))
            .collect::<MiniMutInsertionMap<_, _>>();
        assert_eq!(Some((99, 990)), m.shift_remove_entry(&99));
        assert_eq!(Some(&980), m.get(&98));
        m.assert_invariants();
    }

    #[test]
    fn test_shift_remove_index() {
        let mut m = (0..100)
            .map(|i| (i, i * 10))
            .collect::<MiniMutInsertionMap<_, _>>();
        m.shift_remove_index(5);
        assert_eq!(Some(&40), m.get(&4));
        assert_eq!(None, m.get(&5));
        assert_eq!(Some(&60), m.get(&6));
        m.assert_invariants();
    }

    #[test]
    fn test_json() {
        let mp = mini_mut_insertion_map! {"a".to_owned() => 1, "b".to_owned() => 2};
        let expected = serde_json::json!({
            "a": 1,
            "b": 2,
        });
        assert_eq!(serde_json::to_value(&mp).unwrap(), expected);
        assert!(
            serde_json::from_value::<MiniMutInsertionMap<String, i32>>(expected)
                .unwrap()
                .eq_ordered(&mp)
        );
    }

    #[test]
    fn test_reverse_small() {
        let mut map = MiniMutInsertionMap::new();
        map.insert("a".to_owned(), "b".to_owned());
        map.insert("c".to_owned(), "d".to_owned());
        map.reverse();

        assert_eq!(Some("b"), map.get("a").map(|s| s.as_str()));
        assert_eq!(Some("d"), map.get("c").map(|s| s.as_str()));
        assert_eq!(
            vec![
                ("c".to_owned(), "d".to_owned()),
                ("a".to_owned(), "b".to_owned())
            ],
            map.into_iter().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_reverse_large() {
        let mut map = MiniMutInsertionMap::new();
        for i in 0..100 {
            map.insert(i.to_string(), (i * 10).to_string());
        }

        let expected = map
            .iter()
            .rev()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Vec<_>>();

        map.reverse();
        map.assert_invariants();

        for i in 0..100 {
            assert_eq!(Some(&(i * 10).to_string()), map.get(&i.to_string()));
        }

        assert_eq!(
            expected,
            map.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_retain() {
        let mut map = MiniMutInsertionMap::new();
        for i in 0..100 {
            map.insert(i.to_string(), i);
        }
        map.retain(|_, v| {
            let res = *v % 2 == 0;
            *v += 3;
            res
        });
        assert_eq!(map.len(), 50);
        assert_eq!(map.get("7"), None);
        assert_eq!(map.get("8"), Some(&11));
        map.assert_invariants();
    }

    #[test]
    fn test_and_modify() {
        let mut map = MiniMutInsertionMap::new();
        map.insert("key1", 10);
        map.insert("key3", 100);

        let value1 = map.entry("key1").and_modify(|v| *v += 5).or_insert(0);
        assert_eq!(*value1, 15);
        assert_eq!(map.get("key1"), Some(&15));

        let value2 = map.entry("key2").and_modify(|v| *v += 5).or_insert(10);
        assert_eq!(*value2, 10);
        assert_eq!(map.get("key2"), Some(&10));

        let value3 = map
            .entry("key3")
            .and_modify(|v| *v *= 2)
            .and_modify(|v| *v += 10)
            .or_insert(0);
        assert_eq!(*value3, 210);
        assert_eq!(map.get("key3"), Some(&210));
    }

    #[cfg(feature = "pagable_dep")]
    #[test]
    fn pagable_bytes_embed_stable_hashes() {
        use pagable::PagableSerialize;
        use pagable::testing::TestingSerializer;

        // The serialized form stores each entry's `ShortHash`, so these
        // golden bytes lock the hash function as well as the format. If this fails,
        // previously persisted pagable data no longer matches what a fresh binary
        // computes and must be versioned or invalidated.
        let mut map = MiniMutInsertionMap::new();
        map.insert("name".to_owned(), 1u32);
        map.insert("srcs".to_owned(), 2u32);

        let mut serializer = TestingSerializer::new();
        map.pagable_serialize(&mut serializer)
            .expect("in-memory serialization should not fail");
        assert_eq!(
            serializer.finish(),
            [
                2, 4, 110, 97, 109, 101, 1, 198, 251, 226, 117, 4, 115, 114, 99, 115, 2, 185, 234,
                190, 158, 13
            ],
            "the pagable encoding of `MiniMutInsertionMap` changed (format or hash function); \
             previously persisted pagable data must be versioned or invalidated"
        );
    }

    #[cfg(feature = "pagable_dep")]
    #[test]
    fn pagable_round_trips_with_index() {
        use pagable::PagableDeserialize;
        use pagable::PagableSerialize;

        let map: MiniMutInsertionMap<String, u32> = (0..100).map(|i| (i.to_string(), i)).collect();
        let mut serializer = pagable::testing::TestingSerializer::new();
        map.pagable_serialize(&mut serializer).unwrap();
        let bytes = serializer.finish();
        let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
        let back =
            MiniMutInsertionMap::<String, u32>::pagable_deserialize(&mut deserializer).unwrap();
        assert_eq!(map, back);
        back.assert_invariants();
    }

    /// Ordered by `id` alone, so it can be a key; counts its own drops.
    #[derive(Clone, Hash, PartialEq, Eq)]
    struct Counted {
        id: u32,
        #[allow(dead_code)]
        drops: DropCounter,
    }

    #[derive(Clone)]
    struct DropCounter(Rc<Cell<usize>>);

    impl Hash for DropCounter {
        fn hash<H: Hasher>(&self, _: &mut H) {}
    }
    impl PartialEq for DropCounter {
        fn eq(&self, _: &Self) -> bool {
            true
        }
    }
    impl Eq for DropCounter {}
    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    fn counted(id: u32, drops: &Rc<Cell<usize>>) -> Counted {
        Counted {
            id,
            drops: DropCounter(Rc::clone(drops)),
        }
    }

    /// Every entry is dropped exactly once through every path that gives entries up.
    #[test]
    fn drops_every_entry_exactly_once() {
        for n in [1u32, 5, 40, 100] {
            // Plain drop.
            let drops = Rc::new(Cell::new(0));
            {
                let mut map = MiniMutInsertionMap::new();
                for i in 0..n {
                    map.insert(counted(i, &drops), counted(i, &drops));
                }
            }
            assert_eq!(2 * n as usize, drops.get(), "drop n={n}");

            // Partially consumed owned iterator.
            let drops = Rc::new(Cell::new(0));
            {
                let mut map = MiniMutInsertionMap::new();
                for i in 0..n {
                    map.insert(counted(i, &drops), counted(i, &drops));
                }
                let mut iter = map.into_iter();
                drop(iter.next());
                drop(iter.next_back());
            }
            assert_eq!(2 * n as usize, drops.get(), "into_iter n={n}");

            // Clear, then drop; pop, shift_remove, retain and clone in between.
            let drops = Rc::new(Cell::new(0));
            {
                let mut map = MiniMutInsertionMap::new();
                for i in 0..n {
                    map.insert(counted(i, &drops), counted(i, &drops));
                }
                drop(map.pop());
                drop(map.shift_remove_index(0));
                map.retain(|k, _| k.id % 2 == 0);
                let clone = map.clone();
                map.clear();
                assert!(map.is_empty());
                drop(clone);
                map.insert(counted(1000, &drops), counted(1000, &drops));
            }
            // Each counted value is dropped once, the clones once more: the even ids among
            // `1..n-1` survived `retain`.
            let cloned = n.saturating_sub(2) as usize / 2;
            assert_eq!(2 * (n as usize + 1 + cloned), drops.get(), "mixed n={n}");
        }
    }

    /// A predicate that panics midway leaves the survivors so far, the entry it panicked on and
    /// the ones it never saw, in order, with the index to match.
    #[test]
    fn retain_survives_a_panicking_predicate() {
        for n in [10u32, 100] {
            let drops = Rc::new(Cell::new(0));
            let mut map = MiniMutInsertionMap::new();
            for i in 0..n {
                map.insert(counted(i, &drops), i);
            }
            let result = catch_unwind(AssertUnwindSafe(|| {
                map.retain(|k, _| {
                    assert!(k.id != n / 2, "predicate panic");
                    k.id % 3 != 0
                })
            }));
            assert!(result.is_err());
            let expected: Vec<u32> = (0..n).filter(|i| *i >= n / 2 || i % 3 != 0).collect();
            assert_eq!(expected, map.values().copied().collect::<Vec<_>>(), "n={n}");
            map.assert_invariants();
            assert_eq!(n as usize - expected.len(), drops.get(), "n={n}");
            drop(map);
            assert_eq!(n as usize, drops.get(), "n={n}");
        }
    }

    /// Growth, shrinking and the index coming and going with the capacity.
    #[test]
    fn capacity_management() {
        let mut map = MiniMutInsertionMap::with_capacity(10);
        assert_eq!(10, map.capacity());
        for i in 0..10 {
            map.insert(i, i);
        }
        assert_eq!(10, map.capacity());
        map.insert(10, 10);
        assert!(map.capacity() >= 11);
        map.reserve(1000);
        assert!(map.capacity() >= 1011);
        map.assert_invariants();
        for i in 11..500 {
            map.insert(i, i);
        }
        map.assert_invariants();
        map.shrink_to_fit();
        assert_eq!(500, map.capacity());
        map.assert_invariants();
        for i in 0..500 {
            assert_eq!(Some(&i), map.get(&i));
        }

        while map.len() > 5 {
            map.pop();
        }
        assert_eq!(500, map.capacity());
        map.maybe_drop_index();
        assert_eq!(5, map.capacity());
        map.assert_invariants();
        for i in 0..5 {
            assert_eq!(Some(&i), map.get(&i));
        }

        map.clear();
        assert_eq!(5, map.capacity());
        map.shrink_to_fit();
        assert_eq!(0, map.capacity());
        map.insert(1, 1);
        assert_eq!(Some(&1), map.get(&1));
    }

    /// A long random sequence of operations against a reference model.
    #[test]
    fn matches_a_reference_model() {
        let mut state = 0x1234_5678_9abc_def0u64;
        let mut random = move || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (state >> 33) as u32
        };
        let mut map: MiniMutInsertionMap<u32, u32> = MiniMutInsertionMap::new();
        let mut model: Vec<(u32, u32)> = Vec::new();
        for step in 0..3000 {
            let key = random() % 150;
            match random() % 10 {
                0..=4 => {
                    let value = random();
                    let expected = model.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
                    assert_eq!(expected, map.insert(key, value), "step {step}");
                    match model.iter_mut().find(|(k, _)| *k == key) {
                        Some(entry) => entry.1 = value,
                        None => model.push((key, value)),
                    }
                }
                5 | 6 => {
                    let expected = model.iter().position(|(k, _)| *k == key);
                    let removed = map.shift_remove(&key);
                    assert_eq!(expected.map(|i| model[i].1), removed, "step {step}");
                    if let Some(i) = expected {
                        model.remove(i);
                    }
                }
                7 => {
                    assert_eq!(model.pop(), map.pop(), "step {step}");
                }
                8 => {
                    let keep = random() % 4;
                    map.retain(|k, _| k % 4 != keep);
                    model.retain(|(k, _)| k % 4 != keep);
                }
                _ => {
                    map.reverse();
                    model.reverse();
                }
            }
            if step % 97 == 0 {
                map.assert_invariants();
                assert_eq!(
                    model,
                    map.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>(),
                    "step {step}"
                );
            }
            for &(k, v) in &model {
                assert_eq!(Some(&v), map.get(&k), "step {step} key {k}");
            }
        }
    }

    /// The map and everything that borrows or consumes it move between threads exactly when the
    /// entries do, as for `Vec`.
    #[test]
    fn auto_traits_follow_the_entries() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<MiniMutInsertionMap<String, Vec<u8>>>();
        send_and_sync::<Iter<'static, String, Vec<u8>>>();
        send_and_sync::<IterHashed<'static, String, Vec<u8>>>();
        send_and_sync::<IterMut<'static, String, Vec<u8>>>();
        send_and_sync::<Keys<'static, String, Vec<u8>>>();
        send_and_sync::<Values<'static, String, Vec<u8>>>();
        send_and_sync::<IntoIter<String, Vec<u8>>>();
        send_and_sync::<IntoIterHashed<String, Vec<u8>>>();
        send_and_sync::<IntoKeys<String, Vec<u8>>>();
        send_and_sync::<IntoValues<String, Vec<u8>>>();
    }

    #[test]
    fn handle_is_one_word() {
        mini_vec::size_assert::words_of_type!(MiniMutInsertionMap<u64, u64>, 1);
        mini_vec::size_assert::words_of_type!(MiniMutInsertionMap<String, Vec<u8>>, 1);
    }

    #[test]
    fn allocative_reports_the_whole_allocation() {
        for n in [1usize, 33, 300] {
            let map: MiniMutInsertionMap<u64, u64> = (0..n as u64).map(|i| (i, i)).collect();
            let reported = allocative::size_of_unique_allocated_data(&map);
            assert_eq!(
                layout::offsets::<u64, u64>(map.capacity(), true)
                    .layout
                    .size(),
                reported,
                "n={n}: allocative must account for every byte of the allocation"
            );
        }
    }
}
