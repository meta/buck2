/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Typed access to the regions of an insertion map's allocation.

use std::alloc::Layout;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::slice;

use crate::arrays::Arrays;
use crate::hint::likely;
use crate::insertion_map::index;
use crate::insertion_map::index::TAIL;
use crate::insertion_map::index::Table;
use crate::insertion_map::layout;
use crate::insertion_map::layout::Header;
use crate::scan::find_hash;

/// An allocation laid out by [`offsets`](super::layout::offsets), seen through its regions.
///
/// Holds only what the regions are a function of, the capacity and whether there is a header, and
/// locates each region when asked: a lookup needs three array pointers and nothing else, and this
/// is on the path of every lookup. Carries no length: callers pass the number of initialized
/// entries where it matters, because the mutable map keeps it in the header and a small fixed map
/// keeps it in its handle. Index validity is the caller's job throughout.
pub(crate) struct Raw<K, V> {
    base: *mut u8,
    cap: usize,
    header: bool,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> Clone for Raw<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, V> Copy for Raw<K, V> {}

/// The index of an allocation, at whichever position width its capacity calls for.
pub(crate) enum Index<'a> {
    /// No index: the capacity is within the scan threshold.
    None,
    Narrow(Table<'a, u32>),
    Wide(Table<'a, u64>),
}

/// The lookup through 64-bit positions, which only a map of some billions of entries has. Out of
/// line so that it does not count against inlining the lookup every other map takes.
#[cold]
#[inline(never)]
unsafe fn find_wide(
    tags: &[u8],
    positions: *const u64,
    slots: usize,
    hashes: &[u32],
    hash: u32,
    eq: impl FnMut(usize) -> bool,
) -> Option<usize> {
    // SAFETY: `positions` is the start of `slots` 64-bit positions, per the caller.
    let positions = unsafe { slice::from_raw_parts(positions, slots) };
    index::find(tags, positions, hashes, hash, eq)
}

impl<K, V> Raw<K, V> {
    /// # Safety
    ///
    /// `base` must point at an allocation with the layout of `cap` entries, with or without a
    /// header as `header` says.
    #[inline]
    pub(crate) unsafe fn new(base: NonNull<u8>, cap: usize, header: bool) -> Self {
        Raw {
            base: base.as_ptr(),
            cap,
            header,
            _marker: PhantomData,
        }
    }

    /// A view of no entries at all, backed by nothing; every region is empty.
    #[inline]
    pub(crate) fn dangling() -> Self {
        Raw {
            base: NonNull::dangling().as_ptr(),
            cap: 0,
            header: false,
            _marker: PhantomData,
        }
    }

    #[inline]
    pub(crate) fn base(&self) -> NonNull<u8> {
        // SAFETY: `base` came from a `NonNull`.
        unsafe { NonNull::new_unchecked(self.base) }
    }

    #[inline]
    pub(crate) fn cap(&self) -> usize {
        self.cap
    }

    /// The layout of the whole allocation.
    #[inline]
    pub(crate) fn layout(&self) -> Layout {
        layout::offsets::<K, V>(self.cap, self.header).layout
    }

    #[inline]
    pub(crate) fn arrays(&self) -> Arrays<K, V> {
        if self.cap == 0 {
            // A dangling base cannot be offset into, and there is nothing there to point at.
            return Arrays::dangling();
        }
        let (hashes, keys, values) = layout::array_offsets::<K, V>(self.cap, self.header);
        // SAFETY: the regions are within the allocation, per `new`'s contract.
        unsafe { Arrays::new(self.base, hashes, keys, values) }
    }

    /// The header, which the allocation only has if its layout was made with one.
    #[inline]
    pub(crate) fn header(&self) -> *mut Header {
        debug_assert!(self.header);
        self.base.cast()
    }

    /// The index. Exclusive access is the caller's to guarantee.
    #[inline]
    pub(crate) unsafe fn index<'a>(&self) -> Index<'a> {
        if !layout::has_index(self.cap) {
            return Index::None;
        }
        let region = layout::index_region::<K, V>(self.cap, self.header);
        // SAFETY: the index region is in bounds and was initialized when the map was built, and
        // its position width is the one the layout chose.
        unsafe {
            let tags = slice::from_raw_parts_mut(self.base.add(region.tags), region.slots + TAIL);
            let positions = self.base.add(region.positions);
            if region.wide {
                Index::Wide(Table {
                    tags,
                    positions: slice::from_raw_parts_mut(positions.cast::<u64>(), region.slots),
                })
            } else {
                Index::Narrow(Table {
                    tags,
                    positions: slice::from_raw_parts_mut(positions.cast::<u32>(), region.slots),
                })
            }
        }
    }

    /// Position of the entry with this hash for which `eq` holds, among the first `len` entries.
    #[inline(always)]
    pub(crate) unsafe fn find(
        &self,
        len: usize,
        hash: u32,
        mut eq: impl FnMut(usize) -> bool,
    ) -> Option<usize> {
        // SAFETY: `len` entries are initialized, per the caller.
        let hashes = unsafe { self.arrays().hashes(len) };
        if !layout::has_index(self.cap) {
            let mut i = 0;
            while i < hashes.len() {
                i += find_hash(&hashes[i..], hash)?;
                if likely(eq(i)) {
                    return Some(i);
                }
                i += 1;
            }
            return None;
        }
        let region = layout::index_region::<K, V>(self.cap, self.header);
        // SAFETY: as in `index`; a lookup only reads it.
        unsafe {
            let tags = slice::from_raw_parts(self.base.add(region.tags), region.slots + TAIL);
            let positions = self.base.add(region.positions);
            if region.wide {
                find_wide(
                    tags,
                    positions.cast::<u64>(),
                    region.slots,
                    hashes,
                    hash,
                    eq,
                )
            } else {
                let positions = slice::from_raw_parts(positions.cast::<u32>(), region.slots);
                index::find(tags, positions, hashes, hash, eq)
            }
        }
    }

    /// Record a new entry at `pos` in the index, if there is one.
    #[inline]
    pub(crate) unsafe fn index_insert(&self, hash: u32, pos: usize) {
        match unsafe { self.index() } {
            Index::None => {}
            Index::Narrow(mut table) => table.insert(hash, pos),
            Index::Wide(mut table) => table.insert(hash, pos),
        }
    }

    /// Forget the entry at `pos`, which is still among the first `len` entries, from the index.
    pub(crate) unsafe fn index_remove(&self, len: usize, hash: u32, pos: usize) {
        let hashes = unsafe { self.arrays().hashes(len) };
        match unsafe { self.index() } {
            Index::None => {}
            Index::Narrow(mut table) => table.remove(hashes, hash, pos),
            Index::Wide(mut table) => table.remove(hashes, hash, pos),
        }
    }

    /// Rebuild the index from the first `len` entries' hashes.
    pub(crate) unsafe fn index_rebuild(&self, len: usize) {
        let hashes = unsafe { self.arrays().hashes(len) };
        match unsafe { self.index() } {
            Index::None => {}
            Index::Narrow(mut table) => table.rebuild(hashes),
            Index::Wide(mut table) => table.rebuild(hashes),
        }
    }

    /// Renumber positions after entries moved without changing hashes.
    pub(crate) unsafe fn index_map_positions(&self, f: impl Fn(usize) -> usize) {
        match unsafe { self.index() } {
            Index::None => {}
            Index::Narrow(mut table) => table.map_positions(f),
            Index::Wide(mut table) => table.map_positions(f),
        }
    }

    /// Empty the index.
    pub(crate) unsafe fn index_clear(&self) {
        match unsafe { self.index() } {
            Index::None => {}
            Index::Narrow(mut table) => table.clear(),
            Index::Wide(mut table) => table.clear(),
        }
    }

    /// Everything the index stores, for the test that equal maps are byte-identical.
    #[cfg(test)]
    pub(crate) unsafe fn index_contents(&self) -> (Vec<u8>, Vec<usize>) {
        match unsafe { self.index() } {
            Index::None => (Vec::new(), Vec::new()),
            Index::Narrow(table) => table.contents(),
            Index::Wide(table) => table.contents(),
        }
    }

    /// Check that the index agrees with the first `len` entries.
    #[cfg(test)]
    pub(crate) unsafe fn assert_index_consistent(&self, len: usize) {
        let hashes = unsafe { self.arrays().hashes(len) };
        match unsafe { self.index() } {
            Index::None => {}
            Index::Narrow(table) => table.check(hashes),
            Index::Wide(table) => table.check(hashes),
        }
    }
}
