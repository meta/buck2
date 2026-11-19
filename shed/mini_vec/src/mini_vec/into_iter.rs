/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Iterator returned by [`MiniVec::into_iter`].

use std::iter::FusedIterator;
use std::ptr::NonNull;

use crate::MiniVec;

/// Iterator that moves the elements out of a [`MiniVec`].
///
/// Created by [`MiniVec::into_iter`]. Elements it has not yielded by the time
/// it is dropped are dropped with it.
pub struct IntoIter<T> {
    /// The elements not yet yielded. They live in `_buf`'s allocation, but
    /// this iterator owns them.
    remaining: NonNull<[T]>,
    /// Frees the allocation when dropped, including when dropping an element
    /// panics. Its length is zero so that it does not drop the elements too.
    _buf: MiniVec<T>,
}

// SAFETY: the iterator owns its remaining elements outright, like
// `vec::IntoIter`, so it can cross threads exactly when they can.
unsafe impl<T: Send> Send for IntoIter<T> {}
// SAFETY: `&IntoIter` hands out nothing but the remaining length.
unsafe impl<T: Sync> Sync for IntoIter<T> {}

impl<T> IntoIter<T> {
    #[inline]
    pub(crate) fn new(mut vec: MiniVec<T>) -> Self {
        let unpacked = vec.unpack();
        // SAFETY: zero is within any capacity and leaves no elements that
        // would have to be initialized.
        unsafe {
            vec.set_len(0);
        }
        IntoIter {
            remaining: NonNull::slice_from_raw_parts(unpacked.elements, unpacked.len),
            _buf: vec,
        }
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        let len = self.remaining.len().checked_sub(1)?;
        let first = self.remaining.cast::<T>();
        // SAFETY: `remaining` is not empty, so the other `len` elements start
        // one past `first`, which is at most one past the end of the
        // allocation.
        self.remaining = NonNull::slice_from_raw_parts(unsafe { first.add(1) }, len);
        // SAFETY: `first` was in `remaining`, so it is initialized and owned
        // by this iterator; it has just left `remaining`, so it is not read
        // again.
        Some(unsafe { first.read() })
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.remaining.len();
        (len, Some(len))
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        let len = self.remaining.len().checked_sub(1)?;
        let first = self.remaining.cast::<T>();
        self.remaining = NonNull::slice_from_raw_parts(first, len);
        // SAFETY: as in `next`, for the last element of `remaining`.
        Some(unsafe { first.add(len).read() })
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}
impl<T> FusedIterator for IntoIter<T> {}

impl<T> IntoIter<T> {
    fn drop_impl(&mut self) {
        // SAFETY: the elements in `remaining` are initialized and owned by
        // this iterator, and nothing reads them after its drop.
        unsafe {
            self.remaining.drop_in_place();
        }
    }
}

// SAFETY: `may_dangle` lets an iterator over borrowed data be dropped after
// that data, as `vec::IntoIter` allows, because dropping touches the elements
// only to drop them; see the Rustonomicon on drop check. It is nightly-only,
// so without `rust_nightly` a plain `Drop` impl applies with the stricter
// rules that come with it.
#[cfg(rust_nightly)]
unsafe impl<#[may_dangle] T> Drop for IntoIter<T> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

#[cfg(not(rust_nightly))]
impl<T> Drop for IntoIter<T> {
    fn drop(&mut self) {
        self.drop_impl();
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::MiniVec;

    #[test]
    fn yields_elements_in_order() {
        // Empty, inline and extended.
        for len in [0, 10, 500] {
            let v: MiniVec<u32> = (0..len).collect();
            assert_eq!(
                v.into_iter().collect::<Vec<_>>(),
                (0..len).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn iterates_in_place() {
        // The elements are moved straight out of the vector's allocation, in
        // inline and in extended mode, rather than out of a copy of it.
        for len in [10, 500] {
            let v: MiniVec<u32> = (0..len).collect();
            let elements = v.as_ptr();
            let iter = v.into_iter();
            assert_eq!(iter.remaining.cast::<u32>().as_ptr().cast_const(), elements);
        }
    }

    #[test]
    fn double_ended() {
        let v: MiniVec<u32> = (0..500).collect();
        let mut iter = v.into_iter();
        assert_eq!(iter.len(), 500);
        assert_eq!(iter.next(), Some(0));
        assert_eq!(iter.next_back(), Some(499));
        assert_eq!(iter.next_back(), Some(498));
        assert_eq!(iter.len(), 497);
        assert_eq!(
            iter.rev().collect::<Vec<_>>(),
            (1..498).rev().collect::<Vec<_>>()
        );

        // The two ends meet without yielding an element twice.
        let mut iter = MiniVec::from(vec![1, 2]).into_iter();
        assert_eq!(iter.next_back(), Some(2));
        assert_eq!(iter.next(), Some(1));
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }

    #[test]
    fn drops_unyielded_elements() {
        for len in [10, 500] {
            let r = Rc::new(());
            let v: MiniVec<Rc<()>> = (0..len).map(|_| r.clone()).collect();
            let mut iter = v.into_iter();
            let front = iter.next().unwrap();
            let back = iter.next_back().unwrap();
            assert_eq!(Rc::strong_count(&r), len + 1);
            drop(iter);
            // Only the two elements that were moved out outlive the iterator.
            assert_eq!(Rc::strong_count(&r), 3);
            drop((front, back));
            assert_eq!(Rc::strong_count(&r), 1);
        }
    }

    #[cfg(rust_nightly)]
    #[test]
    fn may_dangle() {
        // Compiles only if drop check lets the iterator outlive what its
        // elements borrow.
        let x = String::new();
        let _iter = MiniVec::from(vec![&x]).into_iter();
        drop(x);
    }
}
