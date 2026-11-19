/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub trait IterExactSize: Sized {
    /// Wraps the iterator so it reports `len` as its exact size and implements
    /// [`ExactSizeIterator`]. For use when the caller knows the exact length of
    /// an iterator (such as a `filter_map` or `chain`) whose own `size_hint`
    /// is not exact.
    ///
    /// `len` must be the exact number of items the iterator will yield; this
    /// is enforced with debug assertions.
    ///
    /// ```
    /// use buck2_util::exact_size_iter::IterExactSize;
    ///
    /// let iter = [1, 2, 3, 4].iter().filter(|x| **x % 2 == 0);
    /// assert_eq!(iter.size_hint(), (0, Some(4)));
    ///
    /// let iter = iter.with_exact_size(2);
    /// assert_eq!(iter.len(), 2);
    /// assert_eq!(iter.collect::<Vec<_>>(), vec![&2, &4]);
    /// ```
    fn with_exact_size(self, len: usize) -> ExactSizeIteratorWrapper<Self>;
}

impl<I: Iterator> IterExactSize for I {
    fn with_exact_size(self, len: usize) -> ExactSizeIteratorWrapper<Self> {
        ExactSizeIteratorWrapper { inner: self, len }
    }
}

/// An iterator reporting an externally supplied exact length. Created by
/// [`with_exact_size`](IterExactSize::with_exact_size).
pub struct ExactSizeIteratorWrapper<I> {
    inner: I,
    len: usize,
}

impl<I: Iterator> Iterator for ExactSizeIteratorWrapper<I> {
    type Item = I::Item;

    #[inline]
    fn next(&mut self) -> Option<I::Item> {
        match self.inner.next() {
            Some(item) => {
                debug_assert!(self.len > 0);
                self.len -= 1;
                Some(item)
            }
            None => {
                debug_assert!(self.len == 0);
                None
            }
        }
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<I: Iterator> ExactSizeIterator for ExactSizeIteratorWrapper<I> {}
