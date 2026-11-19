/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Iterator returned by [`MiniVec::drain`].

use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::ptr;
use std::ptr::NonNull;
use std::slice;

use crate::MiniVec;

/// Draining iterator over a sub-range of a [`MiniVec`].
///
/// Created by [`MiniVec::drain`]. The drained elements are removed from the
/// vector when the iterator is dropped, regardless of whether they were
/// consumed; on drop the tail is shifted into the gap and the vector's
/// length is restored to `original_len - drained_len`.
///
/// # Leak safety
///
/// If a `Drain` is `mem::forget`ed, the elements in the drain range are
/// neither dropped nor exposed: the source vector's length was lowered to
/// the start of the drain range when the iterator was created, so the gap
/// stays sealed off from the live portion of the vector. This matches
/// `std::vec::Drain`'s contract.
pub struct Drain<'a, T> {
    /// Index of the first tail element (i.e. one past the end of the
    /// drain range).
    tail_start: usize,
    /// Number of elements in the tail to preserve.
    tail_len: usize,
    /// Yields the elements being drained.
    iter: slice::Iter<'a, T>,
    /// Pointer back to the source vector. Held as `NonNull` to allow
    /// taking `&mut` in `Drop` while `iter` borrows the elements.
    vec: NonNull<MiniVec<T>>,
    _marker: PhantomData<&'a mut MiniVec<T>>,
}

// SAFETY: `Drain` owns the elements being yielded just like `Vec::Drain`.
unsafe impl<T: Send> Send for Drain<'_, T> {}
unsafe impl<T: Sync> Sync for Drain<'_, T> {}

impl<'a, T> Drain<'a, T> {
    /// Construct a `Drain` over `vec[start..end]`.
    ///
    /// # Safety
    ///
    /// `start <= end <= vec.len()`. The caller must arrange that no other
    /// access to `vec` happens until the returned `Drain` is dropped.
    #[inline]
    pub(crate) unsafe fn new(vec: &'a mut MiniVec<T>, start: usize, end: usize) -> Self {
        let original_len = vec.len();
        // Lower the recorded length so a leaked `Drain` does not expose
        // partially moved-out elements via the source vector.
        // SAFETY: `start <= original_len`; elements `[0..start)` remain
        // initialized.
        unsafe {
            vec.set_len(start);
        }
        // SAFETY: `vec` owns initialized `T`s in `[start..end)`; the slice
        // borrows them for the lifetime of `Drain`, during which the
        // source vector is exclusively borrowed and inaccessible.
        let slice = unsafe { slice::from_raw_parts(vec.as_ptr().add(start), end - start) };
        Drain {
            tail_start: end,
            tail_len: original_len - end,
            iter: slice.iter(),
            vec: NonNull::from(vec),
            _marker: PhantomData,
        }
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        // SAFETY: the slice yields valid references to initialized `T`s
        // we own; we transfer ownership out via `ptr::read`. Subsequent
        // reads of the same slot would be unsound, but the slice iterator
        // does not yield the same element twice.
        self.iter.next().map(|elt| unsafe { ptr::read(elt) })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for Drain<'_, T> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        // SAFETY: see `next`.
        self.iter.next_back().map(|elt| unsafe { ptr::read(elt) })
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {}
impl<T> FusedIterator for Drain<'_, T> {}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        let unread = self.iter.as_slice();
        let unread_len = unread.len();
        let unread_addr = unread.as_ptr().addr();

        // SAFETY: nothing else can be touching `vec` for the duration of
        // this `Drop` because `Drain` exclusively borrows it (`PhantomData`).
        let vec = unsafe { self.vec.as_mut() };
        let base = vec.as_mut_ptr();
        let drain_start = vec.len();

        // Drop any elements still pending in the drain range. `iter` only
        // holds a shared borrow of them, so a pointer taken from it carries
        // read-only provenance and must not be written through; re-derive
        // the pointer from the vector's own storage pointer instead (as
        // `std::vec::Drain` does).
        let unread_ptr = base.with_addr(unread_addr);
        // SAFETY: the unread slice covers initialized `T`s we own; after
        // this `drop_in_place`, they are logically gone and must not be
        // accessed. The source vector's length is below `tail_start`, so
        // it cannot reach them either.
        unsafe {
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(unread_ptr, unread_len));
        }

        // SAFETY: indices `[tail_start..tail_start+tail_len)` and
        // `[drain_start..drain_start+tail_len)` both lie within the
        // allocation; the regions may overlap, hence `ptr::copy`.
        unsafe {
            if self.tail_len > 0 {
                let src = base.add(self.tail_start);
                let dst = base.add(drain_start);
                if src != dst {
                    ptr::copy(src, dst, self.tail_len);
                }
            }
            vec.set_len(drain_start + self.tail_len);
        }
    }
}
