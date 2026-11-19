/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use crate::Hashed;
use crate::iter::def_double_ended_iter;
use crate::iter::def_iter;
use crate::mut_insertion_map;

/// Iterator over the hashed entries of [`MiniMutInsertionSet`](crate::mut_insertion_set::MiniMutInsertionSet).
pub struct Iter<'a, T> {
    pub(crate) iter: mut_insertion_map::Iter<'a, T, ()>,
}

/// Iterator over mutable entries of [`MiniMutInsertionSet`](crate::mut_insertion_set::MiniMutInsertionSet).
pub struct IterMutUnchecked<'a, T> {
    pub(crate) iter: mut_insertion_map::IterMutUnchecked<'a, T, ()>,
}

/// Iterator over the hashed entries of [`MiniMutInsertionSet`](crate::mut_insertion_set::MiniMutInsertionSet).
pub struct IterHashed<'a, T> {
    pub(crate) iter: mut_insertion_map::IterHashed<'a, T, ()>,
}

/// Iterator that moves entries out of a [`MiniMutInsertionSet`](crate::mut_insertion_set::MiniMutInsertionSet).
pub struct IntoIter<T> {
    pub(crate) iter: mut_insertion_map::IntoIter<T, ()>,
}

/// Iterator that moves hashed entries out of a [`MiniMutInsertionSet`](crate::mut_insertion_set::MiniMutInsertionSet).
pub struct IntoIterHashed<T> {
    pub(crate) iter: mut_insertion_map::IntoIterHashed<T, ()>,
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter {
            iter: self.iter.clone(),
        }
    }
}

impl<'a, T> Iter<'a, T> {
    #[inline]
    fn map((k, ()): (&'a T, &'a ())) -> &'a T {
        k
    }
}

impl<'a, T> IterMutUnchecked<'a, T> {
    #[inline]
    fn map((k, ()): (&'a mut T, &'a mut ())) -> &'a mut T {
        k
    }
}

impl<'a, T> IterHashed<'a, T> {
    #[inline]
    fn map((k, ()): (Hashed<&'a T>, &'a ())) -> Hashed<&'a T> {
        k
    }
}

impl<T> IntoIter<T> {
    #[inline]
    fn map((k, ()): (T, ())) -> T {
        k
    }
}

impl<T> IntoIterHashed<T> {
    #[inline]
    fn map((k, ()): (Hashed<T>, ())) -> Hashed<T> {
        k
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    def_iter!();
}

impl<T> DoubleEndedIterator for Iter<'_, T> {
    def_double_ended_iter!();
}

impl<T> ExactSizeIterator for Iter<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<'a, T> Iterator for IterMutUnchecked<'a, T> {
    type Item = &'a mut T;

    def_iter!();
}

impl<T> DoubleEndedIterator for IterMutUnchecked<'_, T> {
    def_double_ended_iter!();
}

impl<T> ExactSizeIterator for IterMutUnchecked<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<'a, T> Iterator for IterHashed<'a, T> {
    type Item = Hashed<&'a T>;

    def_iter!();
}

impl<T> DoubleEndedIterator for IterHashed<'_, T> {
    def_double_ended_iter!();
}

impl<T> ExactSizeIterator for IterHashed<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    def_iter!();
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    def_double_ended_iter!();
}

impl<T> ExactSizeIterator for IntoIter<T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}

impl<T> Iterator for IntoIterHashed<T> {
    type Item = Hashed<T>;

    def_iter!();
}

impl<T> DoubleEndedIterator for IntoIterHashed<T> {
    def_double_ended_iter!();
}

impl<T> ExactSizeIterator for IntoIterHashed<T> {
    #[inline]
    fn len(&self) -> usize {
        self.iter.len()
    }
}
