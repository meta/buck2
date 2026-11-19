/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! [`MiniFixedInsertionSet`], the set built on [`MiniFixedInsertionMap`].

use std::fmt;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;

use allocative::Allocative;
use equivalent::Equivalent;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;
#[cfg(feature = "pagable_dep")]
use pagable::PagableDeserialize;
#[cfg(feature = "pagable_dep")]
use pagable::PagableSerialize;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::Hashed;
use crate::MiniFixedInsertionMap;
use crate::MiniMutInsertionSet;
use crate::fixed_insertion_map::MiniFixedInsertionMapView;
use crate::fixed_insertion_map::iter::Keys;
use crate::mut_insertion_set::IntoIter;
use crate::mut_insertion_set::IntoIterHashed;

/// An immutable set that iterates in insertion order.
///
/// [`MiniFixedInsertionMap`] with unit values, and everything said about that type applies here.
#[derive(Clone)]
#[repr(transparent)]
pub struct MiniFixedInsertionSet<T>(MiniFixedInsertionMap<T, ()>);

/// A read-only [`MiniFixedInsertionSet`] borrowed out of memory somebody else owns.
///
/// The set counterpart of [`MiniFixedInsertionMapView`]; build the bytes with the map's
/// `layout_of` and `init` over unit values.
pub struct MiniFixedInsertionSetView<'a, T>(MiniFixedInsertionMapView<'a, T, ()>);

impl<T> Clone for MiniFixedInsertionSetView<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for MiniFixedInsertionSetView<'_, T> {}

impl<'a, T> MiniFixedInsertionSetView<'a, T> {
    /// Wrap a map view whose values are units.
    #[inline]
    pub fn from_map_view(view: MiniFixedInsertionMapView<'a, T, ()>) -> Self {
        MiniFixedInsertionSetView(view)
    }

    /// Number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` iff there are no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The stored element equal to `value`, if there is one.
    #[inline(always)]
    pub fn get<Q>(&self, value: &Q) -> Option<&'a T>
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.0.get_full(value).map(|(_, t, ())| t)
    }

    /// The stored element equal to `value`, whose hash has already been computed.
    #[inline(always)]
    pub fn get_hashed<Q>(&self, value: Hashed<&Q>) -> Option<&'a T>
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.0.get_full_hashed(value).map(|(_, t, ())| t)
    }

    /// The element at a position.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<&'a T> {
        self.0.get_index(index).map(|(t, ())| t)
    }

    /// The position of an element.
    #[inline(always)]
    pub fn get_index_of<Q>(&self, value: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.0.get_index_of(value)
    }

    /// The position of an element whose hash has already been computed.
    #[inline(always)]
    pub fn get_index_of_hashed<Q>(&self, value: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.0.get_index_of_hashed(value)
    }

    /// Whether the set holds `value`.
    #[inline(always)]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.0.contains_key(value)
    }

    /// Whether the set holds `value`, whose hash has already been computed.
    #[inline(always)]
    pub fn contains_hashed<Q>(&self, value: Hashed<&Q>) -> bool
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.0.contains_key_hashed(value)
    }

    /// Iterate the elements in insertion order.
    #[inline]
    pub fn iter(&self) -> Keys<'a, T, ()> {
        self.0.keys()
    }

    /// Iterate the elements with their stored hashes.
    #[inline]
    pub fn iter_hashed(&self) -> impl ExactSizeIterator<Item = Hashed<&'a T>> {
        self.0.iter_hashed().map(|(t, ())| t)
    }

    /// The first element.
    #[inline]
    pub fn first(&self) -> Option<&'a T> {
        self.get_index(0)
    }

    /// The last element.
    #[inline]
    pub fn last(&self) -> Option<&'a T> {
        self.get_index(self.len().checked_sub(1)?)
    }
}

impl<T> MiniFixedInsertionSet<T> {
    /// An empty set. Does not allocate.
    #[inline]
    pub fn new() -> Self {
        MiniFixedInsertionSet(MiniFixedInsertionMap::new())
    }

    /// Build from pre-hashed elements already known to be distinct, in this order.
    pub fn from_hashed_unique(elements: impl ExactSizeIterator<Item = Hashed<T>>) -> Self {
        MiniFixedInsertionSet(MiniFixedInsertionMap::from_hashed_unique(
            elements.map(|value| (value, ())),
        ))
    }

    /// Number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` iff there are no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrow the set as a view, the type that carries the read API.
    #[inline]
    pub fn view(&self) -> MiniFixedInsertionSetView<'_, T> {
        MiniFixedInsertionSetView(self.0.view())
    }

    /// The stored element equal to `value`, if there is one.
    #[inline(always)]
    pub fn get<Q>(&self, value: &Q) -> Option<&T>
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.view().get(value)
    }

    /// The stored element equal to `value`, whose hash has already been computed.
    #[inline(always)]
    pub fn get_hashed<Q>(&self, value: Hashed<&Q>) -> Option<&T>
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.view().get_hashed(value)
    }

    /// The element at a position.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<&T> {
        self.view().get_index(index)
    }

    /// The position of an element.
    #[inline(always)]
    pub fn get_index_of<Q>(&self, value: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.view().get_index_of(value)
    }

    /// The position of an element whose hash has already been computed.
    #[inline(always)]
    pub fn get_index_of_hashed<Q>(&self, value: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.view().get_index_of_hashed(value)
    }

    /// Whether the set holds `value`.
    #[inline(always)]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.view().contains(value)
    }

    /// Whether the set holds `value`, whose hash has already been computed.
    #[inline(always)]
    pub fn contains_hashed<Q>(&self, value: Hashed<&Q>) -> bool
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.view().contains_hashed(value)
    }

    /// Iterate the elements in insertion order.
    #[inline]
    pub fn iter(&self) -> Keys<'_, T, ()> {
        self.0.keys()
    }

    /// Iterate the elements with their stored hashes.
    #[inline]
    pub fn iter_hashed(&self) -> impl ExactSizeIterator<Item = Hashed<&T>> {
        self.0.iter_hashed().map(|(t, ())| t)
    }

    /// The first element.
    #[inline]
    pub fn first(&self) -> Option<&T> {
        self.view().first()
    }

    /// The last element.
    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.view().last()
    }

    /// Consume the set into its elements with their stored hashes, in insertion order.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<T> {
        IntoIterHashed {
            iter: self.0.into_iter_hashed(),
        }
    }
}

impl<T> Default for MiniFixedInsertionSet<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Debug> Debug for MiniFixedInsertionSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// Order-sensitive: the same elements in the same order.
impl<T: PartialEq> PartialEq for MiniFixedInsertionSet<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq> Eq for MiniFixedInsertionSet<T> {}

impl<T: Hash> Hash for MiniFixedInsertionSet<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

impl<T: StrongHash> StrongHash for MiniFixedInsertionSet<T> {
    #[inline]
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.0.strong_hash(state)
    }
}

impl<T: Allocative> Allocative for MiniFixedInsertionSet<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        self.0.visit(visitor)
    }
}

impl<T: Hash + Eq> FromIterator<T> for MiniFixedInsertionSet<T> {
    /// A repeated element keeps its first position.
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        MiniMutInsertionSet::from_iter(iter).freeze()
    }
}

impl<T: Eq> FromIterator<Hashed<T>> for MiniFixedInsertionSet<T> {
    /// A repeated element keeps its first position.
    fn from_iter<I: IntoIterator<Item = Hashed<T>>>(iter: I) -> Self {
        MiniFixedInsertionSet(iter.into_iter().map(|value| (value, ())).collect())
    }
}

impl<T> From<MiniMutInsertionSet<T>> for MiniFixedInsertionSet<T> {
    #[inline]
    fn from(set: MiniMutInsertionSet<T>) -> Self {
        set.freeze()
    }
}

impl<T> From<MiniFixedInsertionMap<T, ()>> for MiniFixedInsertionSet<T> {
    #[inline]
    fn from(map: MiniFixedInsertionMap<T, ()>) -> Self {
        MiniFixedInsertionSet(map)
    }
}

impl<'a, T> IntoIterator for &'a MiniFixedInsertionSet<T> {
    type Item = &'a T;
    type IntoIter = Keys<'a, T, ()>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> IntoIterator for MiniFixedInsertionSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.0.into_iter(),
        }
    }
}

impl<T: Serialize> Serialize for MiniFixedInsertionSet<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.collect_seq(self.iter())
    }
}

impl<'de, T> Deserialize<'de> for MiniFixedInsertionSet<T>
where
    T: Deserialize<'de> + Hash + Eq,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        MiniMutInsertionSet::deserialize(deserializer).map(MiniMutInsertionSet::freeze)
    }
}

#[cfg(feature = "pagable_dep")]
impl<T: Pagable> PagableSerialize for MiniFixedInsertionSet<T> {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        self.0.pagable_serialize(serializer)
    }
}

#[cfg(feature = "pagable_dep")]
impl<'de, T: Pagable> PagableDeserialize<'de> for MiniFixedInsertionSet<T> {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        Ok(MiniFixedInsertionSet(
            MiniFixedInsertionMap::pagable_deserialize(deserializer)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn into_iter_keeps_the_order() {
        for n in [0u32, 1, 32, 33, 100] {
            let set: MiniFixedInsertionSet<u32> = (0..n).rev().collect();
            assert!(set.clone().into_iter().eq((0..n).rev()));
            assert!(
                set.clone()
                    .into_iter_hashed()
                    .map(|h| (h.hash(), *h.key()))
                    .eq(set.iter_hashed().map(|h| (h.hash(), **h.key())))
            );
        }
    }

    #[test]
    fn lookups_and_positions_across_both_regimes() {
        for n in 0..120u64 {
            let set: MiniFixedInsertionSet<u64> = (0..n).rev().collect();
            assert_eq!(n as usize, set.len());
            for i in 0..n {
                assert!(set.contains(&i), "n={n} i={i}");
                assert_eq!(Some(&i), set.get(&i));
                assert_eq!(Some((n - 1 - i) as usize), set.get_index_of(&i));
                assert_eq!(Some(&i), set.get_index((n - 1 - i) as usize));
            }
            assert!(!set.contains(&n));
            assert_eq!(None, set.get_index(n as usize));
        }
    }

    #[test]
    fn order_matters_for_equality() {
        let forwards: MiniFixedInsertionSet<u64> = (0..200u64).collect();
        let backwards: MiniFixedInsertionSet<u64> = (0..200u64).rev().collect();
        assert_ne!(forwards, backwards);
        assert_eq!(forwards, (0..200u64).collect());
        assert_eq!(
            (0..200u64).rev().collect::<Vec<_>>(),
            backwards.iter().copied().collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicates_keep_their_first_position() {
        let set: MiniFixedInsertionSet<u64> = [3, 1, 2, 1, 3].into_iter().collect();
        assert_eq!(vec![&3, &1, &2], set.iter().collect::<Vec<_>>());
    }

    #[test]
    fn freeze_matches_collect() {
        let mut mutable = MiniMutInsertionSet::new();
        for i in (0..100u64).rev() {
            mutable.insert(i);
        }
        let frozen = mutable.freeze();
        let collected: MiniFixedInsertionSet<u64> = (0..100u64).rev().collect();
        assert_eq!(collected, frozen);
    }

    #[test]
    fn serde_round_trips() {
        let set: MiniFixedInsertionSet<String> =
            (0..30u32).rev().map(|i| format!("e{i}")).collect();
        let json = serde_json::to_string(&set).unwrap();
        assert!(json.starts_with(r#"["e29","e28""#), "{json}");
        let back: MiniFixedInsertionSet<String> = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn handle_is_one_word() {
        mini_vec::size_assert::words_of_type!(MiniFixedInsertionSet<u64>, 1);
    }
}
