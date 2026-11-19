/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! An immutable set whose elements are sorted.

use std::cmp::Ordering;
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
use crate::MiniFixedInsertionSet;
use crate::MiniMutInsertionMap;
use crate::MiniMutInsertionSet;
use crate::fixed_insertion_map::iter::Keys;
use crate::mut_insertion_set::IntoIter;
use crate::mut_insertion_set::IntoIterHashed;

/// An immutable set whose elements are sorted.
///
/// The set counterpart of [`MiniFixedSortedMap`](crate::MiniFixedSortedMap): a
/// [`MiniFixedInsertionSet`] whose elements are in ascending order, so that iteration order,
/// `==`, `Hash`, [`StrongHash`] and the serialized bytes depend on the contents alone. Positions
/// are ranks.
pub struct MiniFixedSortedSet<T>(MiniFixedInsertionSet<T>);

impl<T> MiniFixedSortedSet<T> {
    /// An empty set. Does not allocate.
    #[inline]
    pub fn new() -> Self {
        MiniFixedSortedSet(MiniFixedInsertionSet::new())
    }

    /// The number of elements.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the set has no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The stored element equal to `value`.
    #[inline]
    pub fn get<Q>(&self, value: &Q) -> Option<&T>
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.0.get(value)
    }

    /// The stored element equal to `value`, whose hash has already been computed.
    #[inline]
    pub fn get_hashed<Q>(&self, value: Hashed<&Q>) -> Option<&T>
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.0.get_hashed(value)
    }

    /// The `index`-th smallest element.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<&T> {
        self.0.get_index(index)
    }

    /// The rank of an element: how many elements are smaller than it.
    #[inline]
    pub fn get_index_of<Q>(&self, value: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.0.get_index_of(value)
    }

    /// The rank of an element whose hash has already been computed.
    #[inline]
    pub fn get_index_of_hashed<Q>(&self, value: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.0.get_index_of_hashed(value)
    }

    /// Whether the set holds `value`.
    #[inline]
    pub fn contains<Q>(&self, value: &Q) -> bool
    where
        Q: Hash + Equivalent<T> + ?Sized,
    {
        self.0.contains(value)
    }

    /// Whether the set holds `value`, whose hash has already been computed.
    #[inline]
    pub fn contains_hashed<Q>(&self, value: Hashed<&Q>) -> bool
    where
        Q: Equivalent<T> + ?Sized,
    {
        self.0.contains_hashed(value)
    }

    /// Iterate the elements in ascending order.
    #[inline]
    pub fn iter(&self) -> Keys<'_, T, ()> {
        self.0.iter()
    }

    /// Iterate the elements with their stored hashes, so a rebuild does not have to hash again.
    #[inline]
    pub fn iter_hashed(&self) -> impl ExactSizeIterator<Item = Hashed<&T>> {
        self.0.iter_hashed()
    }

    /// The smallest element.
    #[inline]
    pub fn first(&self) -> Option<&T> {
        self.0.first()
    }

    /// The largest element.
    #[inline]
    pub fn last(&self) -> Option<&T> {
        self.0.last()
    }

    /// Consume the set into its elements with their stored hashes, in order.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<T> {
        self.0.into_iter_hashed()
    }

    /// The same elements as a set that merely promises to keep this order.
    #[inline]
    pub fn into_insertion_set(self) -> MiniFixedInsertionSet<T> {
        self.0
    }
}

impl<T> Default for MiniFixedSortedSet<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Debug> Debug for MiniFixedSortedSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T: Clone> Clone for MiniFixedSortedSet<T> {
    fn clone(&self) -> Self {
        MiniFixedSortedSet(self.0.clone())
    }
}

impl<T: PartialEq> PartialEq for MiniFixedSortedSet<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T: Eq> Eq for MiniFixedSortedSet<T> {}

impl<T: PartialOrd> PartialOrd for MiniFixedSortedSet<T> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

/// Lexicographic over the elements, as for `BTreeSet`.
impl<T: Ord> Ord for MiniFixedSortedSet<T> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<T: Hash> Hash for MiniFixedSortedSet<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

impl<T: StrongHash> StrongHash for MiniFixedSortedSet<T> {
    #[inline]
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.0.strong_hash(state)
    }
}

impl<T: Allocative> Allocative for MiniFixedSortedSet<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        self.0.visit(visitor)
    }
}

impl<T: Ord> From<MiniMutInsertionSet<T>> for MiniFixedSortedSet<T> {
    fn from(mut set: MiniMutInsertionSet<T>) -> Self {
        set.sort();
        MiniFixedSortedSet(set.freeze())
    }
}

impl<T: Hash + Ord> FromIterator<T> for MiniFixedSortedSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Self::from(MiniMutInsertionSet::from_iter(iter))
    }
}

impl<T: Ord> FromIterator<Hashed<T>> for MiniFixedSortedSet<T> {
    fn from_iter<I: IntoIterator<Item = Hashed<T>>>(iter: I) -> Self {
        let mut map: MiniMutInsertionMap<T, ()> =
            iter.into_iter().map(|value| (value, ())).collect();
        map.sort_keys();
        MiniFixedSortedSet(MiniFixedInsertionSet::from(map.freeze()))
    }
}

impl<'a, T> IntoIterator for &'a MiniFixedSortedSet<T> {
    type Item = &'a T;
    type IntoIter = Keys<'a, T, ()>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T> IntoIterator for MiniFixedSortedSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T: Serialize> Serialize for MiniFixedSortedSet<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for MiniFixedSortedSet<T>
where
    T: Deserialize<'de> + Hash + Ord,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        MiniMutInsertionSet::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(feature = "pagable_dep")]
impl<T: Pagable> PagableSerialize for MiniFixedSortedSet<T> {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        self.0.pagable_serialize(serializer)
    }
}

#[cfg(feature = "pagable_dep")]
impl<'de, T: Pagable + Ord> PagableDeserialize<'de> for MiniFixedSortedSet<T> {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        // Serialized by this type, so already sorted; sorting again is a cheap way not to trust
        // bytes with an invariant.
        Ok(Self::from(MiniMutInsertionSet::pagable_deserialize(
            deserializer,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use super::*;

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn ordering_is_lexicographic_over_elements() {
        let set =
            |elements: &[u64]| -> MiniFixedSortedSet<u64> { elements.iter().copied().collect() };
        assert!(set(&[]) < set(&[1]));
        assert!(set(&[1]) < set(&[1, 2]));
        assert!(set(&[1, 2]) < set(&[2]));
        assert_eq!(Ordering::Equal, set(&[2, 1]).cmp(&set(&[1, 2])));
    }

    #[test]
    fn into_iter_is_in_order() {
        for n in [0u64, 1, 32, 33, 100] {
            let set: MiniFixedSortedSet<u64> = (0..n).rev().collect();
            assert!(set.clone().into_iter().eq(0..n));
            assert!(
                set.clone()
                    .into_iter_hashed()
                    .map(|h| (h.hash(), *h.key()))
                    .eq(set.iter_hashed().map(|h| (h.hash(), **h.key())))
            );
        }
    }

    #[test]
    fn sorted_and_canonical_across_both_regimes() {
        for n in [0u64, 1, 2, 16, 32, 33, 100, 1000] {
            let ascending: MiniFixedSortedSet<u64> = (0..n).collect();
            let descending: MiniFixedSortedSet<u64> = (0..n).rev().collect();
            assert_eq!(n as usize, ascending.len());
            assert_eq!(
                (0..n).collect::<Vec<_>>(),
                descending.iter().copied().collect::<Vec<_>>()
            );
            assert_eq!(ascending, descending, "n={n}");
            assert_eq!(hash_of(&ascending), hash_of(&descending), "n={n}");
            for i in 0..n {
                assert!(descending.contains(&i));
                assert_eq!(Some(&i), descending.get(&i));
                assert_eq!(Some(i as usize), descending.get_index_of(&i));
                assert_eq!(Some(&i), descending.get_index(i as usize));
                let hashed = Hashed::new(&i);
                assert!(descending.contains_hashed(hashed));
                assert_eq!(Some(&i), descending.get_hashed(hashed));
                assert_eq!(Some(i as usize), descending.get_index_of_hashed(hashed));
            }
            assert!(!descending.contains(&n));
            if n > 0 {
                assert_eq!(Some(&0), descending.first());
                assert_eq!(Some(&(n - 1)), descending.last());
            }
        }
    }

    #[test]
    fn duplicates_collapse_and_order_is_by_ord() {
        let set: MiniFixedSortedSet<&str> = ["b", "a", "b", "c", "a"].into_iter().collect();
        assert_eq!(vec!["a", "b", "c"], set.iter().copied().collect::<Vec<_>>());
        let mut mutable = MiniMutInsertionSet::new();
        mutable.insert("z".to_owned());
        mutable.insert("y".to_owned());
        let from_mutable = MiniFixedSortedSet::from(mutable);
        assert_eq!(Some(&"y".to_owned()), from_mutable.first());
        let from_hashed: MiniFixedSortedSet<String> = ["z", "y"]
            .into_iter()
            .map(|s| Hashed::new(s.to_owned()))
            .collect();
        assert_eq!(from_mutable, from_hashed);
        assert_eq!(2, from_hashed.into_insertion_set().len());
    }

    #[test]
    fn serde_round_trips_in_order() {
        let set: MiniFixedSortedSet<u32> = [5u32, 1, 3].into_iter().collect();
        let json = serde_json::to_string(&set).unwrap();
        assert_eq!("[1,3,5]", json);
        let back: MiniFixedSortedSet<u32> = serde_json::from_str("[3,2,2,1]").unwrap();
        assert_eq!(vec![1, 2, 3], back.iter().copied().collect::<Vec<_>>());
    }

    #[cfg(feature = "pagable_dep")]
    #[test]
    fn pagable_round_trips() {
        let set: MiniFixedSortedSet<u64> = (0..100u64).rev().collect();
        let mut serializer = pagable::testing::TestingSerializer::new();
        set.pagable_serialize(&mut serializer).unwrap();
        let bytes = serializer.finish();
        let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
        let back = MiniFixedSortedSet::<u64>::pagable_deserialize(&mut deserializer).unwrap();
        assert_eq!(set, back);
    }

    #[test]
    fn auto_traits_follow_the_elements() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<MiniFixedSortedSet<String>>();
    }
}
