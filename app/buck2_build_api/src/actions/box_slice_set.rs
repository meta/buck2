/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::slice;
use std::vec;

use allocative::Allocative;
use buck2_hash::BuckMutInsertionSet;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;

/// A `MiniBoxSlice` whose elements are all unique.
#[derive(Debug, Allocative, Pagable)]
pub struct BoxSliceSet<T>(MiniBoxSlice<T>);

impl<T> BoxSliceSet<T> {
    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.0.iter()
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
}

impl<T> From<BuckMutInsertionSet<T>> for BoxSliceSet<T> {
    #[inline]
    fn from(set: BuckMutInsertionSet<T>) -> BoxSliceSet<T> {
        BoxSliceSet(set.into_iter().collect())
    }
}

impl<T> IntoIterator for BoxSliceSet<T> {
    type Item = T;
    type IntoIter = vec::IntoIter<T>;

    #[inline]
    fn into_iter(self) -> vec::IntoIter<T> {
        self.0.into_vec().into_iter()
    }
}

impl<'a, T> IntoIterator for &'a BoxSliceSet<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> slice::Iter<'a, T> {
        self.iter()
    }
}
