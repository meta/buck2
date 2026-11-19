/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Iterators over insertion-ordered maps, borrowed and owned.

use std::alloc;
use std::alloc::Layout;
use std::iter::Zip;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::slice;

use dupe::Clone_;

use crate::Hashed;
use crate::ShortHash;
use crate::arrays::Arrays;
use crate::iter::def_double_ended_iter;
use crate::iter::def_iter;

/// Iterator over the entries of a map, in insertion order.
#[derive(Clone_)]
pub struct Iter<'a, K, V> {
    iter: Zip<slice::Iter<'a, K>, slice::Iter<'a, V>>,
}

impl<'a, K, V> Iter<'a, K, V> {
    #[inline]
    pub(crate) fn new(keys: &'a [K], values: &'a [V]) -> Self {
        Iter {
            iter: keys.iter().zip(values),
        }
    }

    #[inline]
    fn map(item: (&'a K, &'a V)) -> (&'a K, &'a V) {
        item
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    def_iter!();
}

impl<K, V> ExactSizeIterator for Iter<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for Iter<'_, K, V> {
    def_double_ended_iter!();
}

/// Iterator over the entries of a map together with their stored hashes.
#[derive(Clone_)]
pub struct IterHashed<'a, K, V> {
    iter: Zip<Zip<slice::Iter<'a, K>, slice::Iter<'a, V>>, slice::Iter<'a, u32>>,
}

impl<'a, K, V> IterHashed<'a, K, V> {
    #[inline]
    pub(crate) fn new(keys: &'a [K], values: &'a [V], hashes: &'a [u32]) -> Self {
        IterHashed {
            iter: keys.iter().zip(values).zip(hashes),
        }
    }

    #[inline]
    fn map(item: ((&'a K, &'a V), &'a u32)) -> (Hashed<&'a K>, &'a V) {
        let ((key, value), hash) = item;
        (
            Hashed::new_unchecked(ShortHash::new_unchecked(*hash), key),
            value,
        )
    }
}

impl<'a, K, V> Iterator for IterHashed<'a, K, V> {
    type Item = (Hashed<&'a K>, &'a V);

    def_iter!();
}

impl<K, V> ExactSizeIterator for IterHashed<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IterHashed<'_, K, V> {
    def_double_ended_iter!();
}

/// Iterator over the entries of a map with mutable values.
pub struct IterMut<'a, K, V> {
    iter: Zip<slice::Iter<'a, K>, slice::IterMut<'a, V>>,
}

impl<'a, K, V> IterMut<'a, K, V> {
    #[inline]
    pub(crate) fn new(keys: &'a [K], values: &'a mut [V]) -> Self {
        IterMut {
            iter: keys.iter().zip(values),
        }
    }

    #[inline]
    fn map(item: (&'a K, &'a mut V)) -> (&'a K, &'a mut V) {
        item
    }
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    def_iter!();
}

impl<K, V> ExactSizeIterator for IterMut<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IterMut<'_, K, V> {
    def_double_ended_iter!();
}

/// Iterator over the entries of a map with mutable keys and values.
///
/// Memory safe, but a key mutated so that its hash or equality changes is no longer found.
pub struct IterMutUnchecked<'a, K, V> {
    iter: Zip<slice::IterMut<'a, K>, slice::IterMut<'a, V>>,
}

impl<'a, K, V> IterMutUnchecked<'a, K, V> {
    #[inline]
    pub(crate) fn new(keys: &'a mut [K], values: &'a mut [V]) -> Self {
        IterMutUnchecked {
            iter: keys.iter_mut().zip(values),
        }
    }

    #[inline]
    fn map(item: (&'a mut K, &'a mut V)) -> (&'a mut K, &'a mut V) {
        item
    }
}

impl<'a, K, V> Iterator for IterMutUnchecked<'a, K, V> {
    type Item = (&'a mut K, &'a mut V);

    def_iter!();
}

impl<K, V> ExactSizeIterator for IterMutUnchecked<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IterMutUnchecked<'_, K, V> {
    def_double_ended_iter!();
}

/// Iterator over the keys of a map, in insertion order.
#[derive(Clone_)]
pub struct Keys<'a, K, V> {
    iter: slice::Iter<'a, K>,
    _marker: PhantomData<&'a V>,
}

impl<'a, K, V> Keys<'a, K, V> {
    #[inline]
    pub(crate) fn new(keys: &'a [K]) -> Self {
        Keys {
            iter: keys.iter(),
            _marker: PhantomData,
        }
    }

    #[inline]
    fn map(key: &'a K) -> &'a K {
        key
    }
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;

    def_iter!();
}

impl<K, V> ExactSizeIterator for Keys<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for Keys<'_, K, V> {
    def_double_ended_iter!();
}

/// Iterator over the values of a map, in insertion order.
#[derive(Clone_)]
pub struct Values<'a, K, V> {
    iter: slice::Iter<'a, V>,
    _marker: PhantomData<&'a K>,
}

impl<'a, K, V> Values<'a, K, V> {
    #[inline]
    pub(crate) fn new(values: &'a [V]) -> Self {
        Values {
            iter: values.iter(),
            _marker: PhantomData,
        }
    }

    #[inline]
    fn map(value: &'a V) -> &'a V {
        value
    }
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;

    def_iter!();
}

impl<K, V> ExactSizeIterator for Values<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for Values<'_, K, V> {
    def_double_ended_iter!();
}

/// Iterator over the values of a map, mutably, in insertion order.
pub struct ValuesMut<'a, K, V> {
    iter: slice::IterMut<'a, V>,
    _marker: PhantomData<&'a K>,
}

impl<'a, K, V> ValuesMut<'a, K, V> {
    #[inline]
    pub(crate) fn new(values: &'a mut [V]) -> Self {
        ValuesMut {
            iter: values.iter_mut(),
            _marker: PhantomData,
        }
    }

    #[inline]
    fn map(value: &'a mut V) -> &'a mut V {
        value
    }
}

impl<'a, K, V> Iterator for ValuesMut<'a, K, V> {
    type Item = &'a mut V;

    def_iter!();
}

impl<K, V> ExactSizeIterator for ValuesMut<'_, K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for ValuesMut<'_, K, V> {
    def_double_ended_iter!();
}

/// Moves the entries out of a map's allocation, front to back or back to front, dropping the ones
/// nobody asked for and freeing the block at the end.
pub(crate) struct OwnedEntries<K, V> {
    arrays: Arrays<K, V>,
    front: usize,
    back: usize,
    /// The block to free, if the map had one.
    block: Option<(NonNull<u8>, Layout)>,
}

// SAFETY: the iterator owns its remaining entries outright, like `vec::IntoIter`, so it can cross
// threads exactly when they can.
unsafe impl<K: Send, V: Send> Send for OwnedEntries<K, V> {}
// SAFETY: `&OwnedEntries` hands out nothing but the remaining length.
unsafe impl<K: Sync, V: Sync> Sync for OwnedEntries<K, V> {}

impl<K, V> OwnedEntries<K, V> {
    /// # Safety
    ///
    /// `arrays` must hold `len` initialized entries owned by nobody else, in `block` if given,
    /// which must then have been allocated with that layout.
    pub(crate) unsafe fn new(
        arrays: Arrays<K, V>,
        len: usize,
        block: Option<(NonNull<u8>, Layout)>,
    ) -> Self {
        OwnedEntries {
            arrays,
            front: 0,
            back: len,
            block,
        }
    }

    #[inline]
    fn len(&self) -> usize {
        self.back - self.front
    }
}

impl<K, V> Iterator for OwnedEntries<K, V> {
    type Item = (u32, K, V);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        // SAFETY: entries `front..back` are initialized and owned by this iterator; the one read
        // leaves that range.
        let entry = unsafe { self.arrays.read(self.front) };
        self.front += 1;
        Some(entry)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len(), Some(self.len()))
    }
}

impl<K, V> DoubleEndedIterator for OwnedEntries<K, V> {
    #[inline]
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        // SAFETY: as in `next`.
        Some(unsafe { self.arrays.read(self.back) })
    }
}

impl<K, V> ExactSizeIterator for OwnedEntries<K, V> {}

impl<K, V> OwnedEntries<K, V> {
    fn drop_impl(&mut self) {
        // SAFETY: entries `front..back` are the ones not yet moved out.
        unsafe {
            self.arrays
                .offset(self.front)
                .drop_entries(self.back - self.front);
        }
        if let Some((ptr, layout)) = self.block {
            // SAFETY: the block was allocated with this layout and nothing refers to it any more.
            unsafe { alloc::dealloc(ptr.as_ptr(), layout) };
        }
    }
}

// SAFETY: `may_dangle` lets a map holding borrowed data be dropped after that data, as `Vec`
// allows, because dropping touches the entries only to drop them; see the Rustonomicon on drop
// check. It is nightly-only, so without `rust_nightly` a plain `Drop` impl applies with the stricter
// rules that come with it.
#[cfg(rust_nightly)]
unsafe impl<#[may_dangle] K, #[may_dangle] V> Drop for OwnedEntries<K, V> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

#[cfg(not(rust_nightly))]
impl<K, V> Drop for OwnedEntries<K, V> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

/// Iterator that moves the entries out of a map together with their stored hashes.
pub struct IntoIterHashed<K, V> {
    iter: OwnedEntries<K, V>,
}

impl<K, V> IntoIterHashed<K, V> {
    #[inline]
    pub(crate) fn new(entries: OwnedEntries<K, V>) -> Self {
        IntoIterHashed { iter: entries }
    }

    #[inline]
    fn map((hash, key, value): (u32, K, V)) -> (Hashed<K>, V) {
        (
            Hashed::new_unchecked(ShortHash::new_unchecked(hash), key),
            value,
        )
    }
}

impl<K, V> Iterator for IntoIterHashed<K, V> {
    type Item = (Hashed<K>, V);

    def_iter!();
}

impl<K, V> ExactSizeIterator for IntoIterHashed<K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IntoIterHashed<K, V> {
    def_double_ended_iter!();
}

/// Iterator that moves the entries out of a map.
pub struct IntoIter<K, V> {
    iter: OwnedEntries<K, V>,
}

impl<K, V> IntoIter<K, V> {
    #[inline]
    pub(crate) fn new(entries: OwnedEntries<K, V>) -> Self {
        IntoIter { iter: entries }
    }

    #[inline]
    fn map((_, key, value): (u32, K, V)) -> (K, V) {
        (key, value)
    }
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);

    def_iter!();
}

impl<K, V> ExactSizeIterator for IntoIter<K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IntoIter<K, V> {
    def_double_ended_iter!();
}

/// Iterator that moves the keys out of a map.
pub struct IntoKeys<K, V> {
    iter: OwnedEntries<K, V>,
}

impl<K, V> IntoKeys<K, V> {
    #[inline]
    pub(crate) fn new(entries: OwnedEntries<K, V>) -> Self {
        IntoKeys { iter: entries }
    }

    #[inline]
    fn map((_, key, _): (u32, K, V)) -> K {
        key
    }
}

impl<K, V> Iterator for IntoKeys<K, V> {
    type Item = K;

    def_iter!();
}

impl<K, V> ExactSizeIterator for IntoKeys<K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IntoKeys<K, V> {
    def_double_ended_iter!();
}

/// Iterator that moves the values out of a map.
pub struct IntoValues<K, V> {
    iter: OwnedEntries<K, V>,
}

impl<K, V> IntoValues<K, V> {
    #[inline]
    pub(crate) fn new(entries: OwnedEntries<K, V>) -> Self {
        IntoValues { iter: entries }
    }

    #[inline]
    fn map((_, _, value): (u32, K, V)) -> V {
        value
    }
}

impl<K, V> Iterator for IntoValues<K, V> {
    type Item = V;

    def_iter!();
}

impl<K, V> ExactSizeIterator for IntoValues<K, V> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<K, V> DoubleEndedIterator for IntoValues<K, V> {
    def_double_ended_iter!();
}
