/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The parallel entry arrays every map in this crate stores its entries in: a hash, a key and a
//! value at the same index of three arrays, and the guards that keep construction panic-safe.

use std::alloc;
use std::alloc::Layout;
use std::ptr;
use std::ptr::NonNull;
use std::slice;

/// The three entry arrays of an allocation, as raw pointers.
///
/// Everything that writes, permutes or drops entries goes through this type. Index validity is
/// the caller's job throughout: every index must be below the entry count the arrays were sized
/// for, and reads require the entry at that index to be initialized.
pub(crate) struct Arrays<K, V> {
    hashes: *mut u32,
    keys: *mut K,
    values: *mut V,
}

impl<K, V> Clone for Arrays<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K, V> Copy for Arrays<K, V> {}

impl<K, V> Arrays<K, V> {
    /// # Safety
    ///
    /// `base` must point at an allocation in which the three arrays start at the given byte
    /// offsets, each suitably aligned.
    #[inline]
    pub(crate) unsafe fn new(base: *mut u8, hashes: usize, keys: usize, values: usize) -> Self {
        unsafe {
            Arrays {
                hashes: base.add(hashes).cast(),
                keys: base.add(keys).cast(),
                values: base.add(values).cast(),
            }
        }
    }

    /// Arrays of zero entries, valid for every operation that touches no entry.
    #[inline]
    pub(crate) fn dangling() -> Self {
        Arrays {
            hashes: NonNull::dangling().as_ptr(),
            keys: NonNull::dangling().as_ptr(),
            values: NonNull::dangling().as_ptr(),
        }
    }

    /// The same arrays, starting `start` entries in.
    #[inline]
    pub(crate) unsafe fn offset(self, start: usize) -> Self {
        unsafe {
            Arrays {
                hashes: self.hashes.add(start),
                keys: self.keys.add(start),
                values: self.values.add(start),
            }
        }
    }

    #[inline]
    pub(crate) unsafe fn write(self, i: usize, hash: u32, key: K, value: V) {
        unsafe {
            ptr::write(self.hashes.add(i), hash);
            ptr::write(self.keys.add(i), key);
            ptr::write(self.values.add(i), value);
        }
    }

    /// Move entry `i` out, leaving its slot uninitialized.
    #[inline]
    pub(crate) unsafe fn read(self, i: usize) -> (u32, K, V) {
        unsafe {
            (
                *self.hashes.add(i),
                ptr::read(self.keys.add(i)),
                ptr::read(self.values.add(i)),
            )
        }
    }

    #[inline]
    pub(crate) unsafe fn key<'a>(self, i: usize) -> &'a K {
        unsafe { &*self.keys.add(i) }
    }

    #[inline]
    pub(crate) unsafe fn value<'a>(self, i: usize) -> &'a V {
        unsafe { &*self.values.add(i) }
    }

    #[inline]
    pub(crate) unsafe fn value_mut<'a>(self, i: usize) -> &'a mut V {
        unsafe { &mut *self.values.add(i) }
    }

    /// The stored hashes of the first `len` entries.
    #[inline]
    pub(crate) unsafe fn hashes<'a>(self, len: usize) -> &'a [u32] {
        unsafe { slice::from_raw_parts(self.hashes, len) }
    }

    #[inline]
    pub(crate) unsafe fn hashes_mut<'a>(self, len: usize) -> &'a mut [u32] {
        unsafe { slice::from_raw_parts_mut(self.hashes, len) }
    }

    #[inline]
    pub(crate) unsafe fn keys<'a>(self, len: usize) -> &'a [K] {
        unsafe { slice::from_raw_parts(self.keys, len) }
    }

    #[inline]
    pub(crate) unsafe fn keys_mut<'a>(self, len: usize) -> &'a mut [K] {
        unsafe { slice::from_raw_parts_mut(self.keys, len) }
    }

    #[inline]
    pub(crate) unsafe fn values<'a>(self, len: usize) -> &'a [V] {
        unsafe { slice::from_raw_parts(self.values, len) }
    }

    #[inline]
    pub(crate) unsafe fn values_mut<'a>(self, len: usize) -> &'a mut [V] {
        unsafe { slice::from_raw_parts_mut(self.values, len) }
    }

    /// Move `count` entries starting at `from` to start at `to`; the ranges may overlap.
    #[inline]
    pub(crate) unsafe fn shift(self, from: usize, to: usize, count: usize) {
        unsafe {
            ptr::copy(self.hashes.add(from), self.hashes.add(to), count);
            ptr::copy(self.keys.add(from), self.keys.add(to), count);
            ptr::copy(self.values.add(from), self.values.add(to), count);
        }
    }

    /// Bitwise copy of the first `count` entries of `src` into these arrays, which must not
    /// overlap it.
    #[inline]
    pub(crate) unsafe fn copy_from(self, src: Self, count: usize) {
        unsafe {
            ptr::copy_nonoverlapping(src.hashes, self.hashes, count);
            ptr::copy_nonoverlapping(src.keys, self.keys, count);
            ptr::copy_nonoverlapping(src.values, self.values, count);
        }
    }

    /// Reverse the order of the first `len` entries.
    pub(crate) unsafe fn reverse(self, len: usize) {
        unsafe {
            self.hashes_mut(len).reverse();
            self.keys_mut(len).reverse();
            self.values_mut(len).reverse();
        }
    }

    /// Drop the first `len` keys and values.
    pub(crate) unsafe fn drop_entries(self, len: usize) {
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.keys, len));
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(self.values, len));
        }
    }

    /// Rearrange the entries so that the one now at `order[j]` ends up at `j`, for every `j`.
    ///
    /// # Safety
    ///
    /// `order` must be a permutation of `0..order.len()`, and that many entries must be
    /// initialized. Cannot panic. Leaves `order` as the identity permutation.
    pub(crate) unsafe fn permute(self, order: &mut [usize]) {
        for j in 0..order.len() {
            let mut src = order[j];
            if src == j {
                continue;
            }
            // SAFETY: every index is below the length. Along a cycle each entry is read once and
            // written once into the hole the previous read left, the one in hand last, so no
            // entry is duplicated or lost.
            unsafe {
                let (hash, key, value) = self.read(j);
                let mut hole = j;
                while src != j {
                    let (h, k, v) = self.read(src);
                    self.write(hole, h, k, v);
                    order[hole] = hole;
                    hole = src;
                    src = order[hole];
                }
                self.write(hole, hash, key, value);
                order[hole] = hole;
            }
        }
    }
}

/// Drops the entries written so far if construction unwinds, so that a panic leaves the memory
/// uninitialized again rather than leaking what was already in it.
pub(crate) struct InitGuard<K, V> {
    arrays: Arrays<K, V>,
    /// Entries `..written` are initialized in all three arrays.
    pub(crate) written: usize,
}

impl<K, V> InitGuard<K, V> {
    pub(crate) fn new(arrays: Arrays<K, V>) -> Self {
        InitGuard { arrays, written: 0 }
    }
}

impl<K, V> Drop for InitGuard<K, V> {
    fn drop(&mut self) {
        // SAFETY: exactly `written` entries are initialized, by this type's contract.
        unsafe { self.arrays.drop_entries(self.written) }
    }
}

/// Frees the block when dropped, so that it is freed even if dropping or cloning an entry panics,
/// matching `Vec`.
pub(crate) struct DeallocGuard {
    pub(crate) ptr: NonNull<u8>,
    pub(crate) layout: Layout,
}

impl Drop for DeallocGuard {
    fn drop(&mut self) {
        // SAFETY: the block was allocated with exactly this layout and is not used again.
        unsafe { alloc::dealloc(self.ptr.as_ptr(), self.layout) };
    }
}

/// Allocate a non-zero-sized block, aborting on failure as std's collections do.
pub(crate) fn alloc_block(layout: Layout) -> NonNull<u8> {
    debug_assert!(layout.size() > 0);
    // SAFETY: the layout is non-zero-sized.
    let ptr = unsafe { alloc::alloc(layout) };
    match NonNull::new(ptr) {
        Some(ptr) => ptr,
        None => alloc::handle_alloc_error(layout),
    }
}
