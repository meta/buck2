/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Borrow;
use std::slice;

use allocative::Allocative;
use mini_vec::MiniBoxSlice;
use pagable::Pagable;

/// An immutable sorted key-value map with a single-word stack footprint and an exactly-sized
/// allocation.
///
/// This is the entry storage of fingerprinted directories, which exist in large enough numbers
/// that a one-word handle with no excess capacity is worth the binary-search lookups.
#[derive(Clone, Debug, Allocative, Pagable)]
pub struct SortedSliceMap<K, V> {
    entries: MiniBoxSlice<(K, V)>,
}

impl<K, V> SortedSliceMap<K, V> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get<Q>(&self, q: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.entries
            .binary_search_by(|(k, _)| k.borrow().cmp(q))
            .ok()
            .map(|i| &self.entries[i].1)
    }

    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        self.entries.as_slice().get(index).map(|(k, v)| (k, v))
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter(self.entries.iter())
    }
}

/// Collects entries that must already be sorted by strictly ascending key; this is
/// debug-asserted, not checked in release builds.
impl<K: Ord, V> FromIterator<(K, V)> for SortedSliceMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let entries = MiniBoxSlice::from_iter(iter);
        debug_assert!(
            entries.windows(2).all(|w| w[0].0 < w[1].0),
            "SortedSliceMap must be collected from strictly ascending keys",
        );
        Self { entries }
    }
}

impl<'a, K, V> IntoIterator for &'a SortedSliceMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V> IntoIterator for SortedSliceMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter(self.entries.into_vec().into_iter())
    }
}

pub struct Iter<'a, K, V>(slice::Iter<'a, (K, V)>);

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|(k, v)| (k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<'a, K, V> ExactSizeIterator for Iter<'a, K, V> {}

pub struct IntoIter<K, V>(std::vec::IntoIter<(K, V)>);

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl<K, V> ExactSizeIterator for IntoIter<K, V> {}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(mut pairs: Vec<(u32, &'static str)>) -> SortedSliceMap<u32, &'static str> {
        pairs.sort_by_key(|(k, _)| *k);
        pairs.dedup_by_key(|(k, _)| *k);
        SortedSliceMap::from_iter(pairs)
    }

    #[test]
    fn test_get() {
        let m = map(vec![(3, "c"), (1, "a"), (2, "b")]);
        assert_eq!(m.len(), 3);
        assert_eq!(m.get(&1), Some(&"a"));
        assert_eq!(m.get(&3), Some(&"c"));
        assert_eq!(m.get(&4), None);
    }

    #[test]
    fn test_iter_sorted() {
        let m = map(vec![(2, "b"), (1, "a")]);
        let keys: Vec<u32> = m.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec![1, 2]);
        let owned: Vec<(u32, &str)> = m.into_iter().collect();
        assert_eq!(owned, vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn test_get_index() {
        let m = map(vec![(1, "a"), (2, "b")]);
        assert_eq!(m.get_index(1), Some((&2, &"b")));
        assert_eq!(m.get_index(2), None);
    }

    #[test]
    fn test_empty() {
        let m: SortedSliceMap<u32, ()> = SortedSliceMap::from_iter([]);
        assert!(m.is_empty());
        assert_eq!(m.get(&1), None);
        assert_eq!(m.iter().next(), None);
    }

    #[test]
    fn test_size() {
        assert_eq!(
            std::mem::size_of::<SortedSliceMap<u32, String>>(),
            std::mem::size_of::<usize>(),
        );
    }
}
