/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A map whose keys are fixed at construction and sorted.

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
use crate::MiniFixedInsertionMap;
use crate::MiniMutInsertionMap;
use crate::fixed_insertion_map::iter::IntoIter;
use crate::fixed_insertion_map::iter::IntoIterHashed;
use crate::fixed_insertion_map::iter::IntoKeys;
use crate::fixed_insertion_map::iter::IntoValues;
use crate::fixed_insertion_map::iter::Iter;
use crate::fixed_insertion_map::iter::IterHashed;
use crate::fixed_insertion_map::iter::IterMut;
use crate::fixed_insertion_map::iter::Keys;
use crate::fixed_insertion_map::iter::Values;
use crate::fixed_insertion_map::iter::ValuesMut;

/// A map whose keys are fixed at construction and sorted.
///
/// A [`MiniFixedInsertionMap`] with one more invariant: the entries are in ascending key order. So
/// the iteration order, and with it `==`, `Hash`, [`StrongHash`] and the serialized bytes, are
/// functions of the contents alone, however the map was built. It is the map for data that is
/// built once and kept, when nobody needs the order the entries arrived in. Sorting by `Ord`
/// rather than by hash keeps the order readable and stable across hasher and binary versions, so
/// the map can feed digests that have to survive both.
///
/// Positions are ranks: [`get_index(0)`](Self::get_index) is the smallest key. When a key repeats
/// during construction the last value wins, as it would for a sequence of inserts. Values can be
/// replaced in place afterwards; keys and their ranks cannot change.
pub struct MiniFixedSortedMap<K, V>(MiniFixedInsertionMap<K, V>);

impl<K, V> MiniFixedSortedMap<K, V> {
    /// An empty map. Does not allocate.
    #[inline]
    pub fn new() -> Self {
        MiniFixedSortedMap(MiniFixedInsertionMap::new())
    }

    /// The number of entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the map has no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Look up a key whose hash has already been computed.
    #[inline]
    pub fn get_hashed<Q>(&self, key: Hashed<&Q>) -> Option<&V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.0.get_hashed(key)
    }

    /// Look up a key.
    #[inline]
    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.0.get(key)
    }

    /// Look up a key, returning its rank and the stored key alongside its value.
    #[inline]
    pub fn get_full<Q>(&self, key: &Q) -> Option<(usize, &K, &V)>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.0.get_full(key)
    }

    /// Look up a key whose hash has already been computed, returning its rank and the stored key
    /// alongside its value.
    #[inline]
    pub fn get_full_hashed<Q>(&self, key: Hashed<&Q>) -> Option<(usize, &K, &V)>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.0.get_full_hashed(key)
    }

    /// The entry with the `index`-th smallest key.
    #[inline]
    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        self.0.get_index(index)
    }

    /// The rank of a key: how many keys are smaller than it.
    #[inline]
    pub fn get_index_of<Q>(&self, key: &Q) -> Option<usize>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.0.get_index_of(key)
    }

    /// The rank of a key whose hash has already been computed.
    #[inline]
    pub fn get_index_of_hashed<Q>(&self, key: Hashed<&Q>) -> Option<usize>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.0.get_index_of_hashed(key)
    }

    /// Whether the map holds `key`.
    #[inline]
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.0.contains_key(key)
    }

    /// Whether the map holds `key`, whose hash has already been computed.
    #[inline]
    pub fn contains_key_hashed<Q>(&self, key: Hashed<&Q>) -> bool
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.0.contains_key_hashed(key)
    }

    /// Iterate the entries in ascending key order.
    #[inline]
    pub fn iter(&self) -> Iter<'_, K, V> {
        self.0.iter()
    }

    /// Iterate the entries with their stored hashes, so a rebuild does not have to hash again.
    #[inline]
    pub fn iter_hashed(&self) -> IterHashed<'_, K, V> {
        self.0.iter_hashed()
    }

    /// Iterate the keys in ascending order.
    #[inline]
    pub fn keys(&self) -> Keys<'_, K, V> {
        self.0.keys()
    }

    /// Iterate the values in ascending order of their keys.
    #[inline]
    pub fn values(&self) -> Values<'_, K, V> {
        self.0.values()
    }

    /// The entry with the smallest key.
    #[inline]
    pub fn first(&self) -> Option<(&K, &V)> {
        self.0.first()
    }

    /// The entry with the largest key.
    #[inline]
    pub fn last(&self) -> Option<(&K, &V)> {
        self.0.last()
    }

    /// Look up a key whose hash has already been computed, for writing to its value.
    #[inline(always)]
    pub fn get_mut_hashed<Q>(&mut self, key: Hashed<&Q>) -> Option<&mut V>
    where
        Q: Equivalent<K> + ?Sized,
    {
        self.0.get_mut_hashed(key)
    }

    /// Look up a key, for writing to its value.
    #[inline(always)]
    pub fn get_mut<Q>(&mut self, key: &Q) -> Option<&mut V>
    where
        Q: Hash + Equivalent<K> + ?Sized,
    {
        self.0.get_mut(key)
    }

    /// Iterate the entries in key order, with the values writable.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        self.0.iter_mut()
    }

    /// Iterate the values in key order, writable.
    #[inline]
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        self.0.values_mut()
    }

    /// Consume the map into its entries with their stored hashes, in key order.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<K, V> {
        self.0.into_iter_hashed()
    }

    /// Consume the map into its keys, in order.
    #[inline]
    pub fn into_keys(self) -> IntoKeys<K, V> {
        self.0.into_keys()
    }

    /// Consume the map into its values, in key order.
    #[inline]
    pub fn into_values(self) -> IntoValues<K, V> {
        self.0.into_values()
    }

    /// The same entries as a map that merely promises to keep this order.
    #[inline]
    pub fn into_insertion_map(self) -> MiniFixedInsertionMap<K, V> {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn assert_invariants(&self)
    where
        K: Ord,
    {
        self.0.assert_invariants();
        let keys: Vec<&K> = self.keys().collect();
        assert!(
            keys.windows(2).all(|w| w[0] < w[1]),
            "keys must be strictly ascending"
        );
    }
}

impl<K, V> Default for MiniFixedSortedMap<K, V> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Debug, V: Debug> Debug for MiniFixedSortedMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Clone, V: Clone> Clone for MiniFixedSortedMap<K, V> {
    fn clone(&self) -> Self {
        MiniFixedSortedMap(self.0.clone())
    }
}

impl<K: PartialEq, V: PartialEq> PartialEq for MiniFixedSortedMap<K, V> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<K: Eq, V: Eq> Eq for MiniFixedSortedMap<K, V> {}

impl<K: PartialOrd, V: PartialOrd> PartialOrd for MiniFixedSortedMap<K, V> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

/// Lexicographic over the entries, as for `BTreeMap`.
impl<K: Ord, V: Ord> Ord for MiniFixedSortedMap<K, V> {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<K: Hash, V: Hash> Hash for MiniFixedSortedMap<K, V> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}

impl<K: StrongHash, V: StrongHash> StrongHash for MiniFixedSortedMap<K, V> {
    #[inline]
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.0.strong_hash(state)
    }
}

impl<K: Allocative, V: Allocative> Allocative for MiniFixedSortedMap<K, V> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        self.0.visit(visitor)
    }
}

impl<K: Ord, V> From<MiniMutInsertionMap<K, V>> for MiniFixedSortedMap<K, V> {
    fn from(mut map: MiniMutInsertionMap<K, V>) -> Self {
        map.sort_keys();
        MiniFixedSortedMap(map.freeze())
    }
}

impl<K: Hash + Ord, V> FromIterator<(K, V)> for MiniFixedSortedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self::from(MiniMutInsertionMap::from_iter(iter))
    }
}

impl<K: Ord, V> FromIterator<(Hashed<K>, V)> for MiniFixedSortedMap<K, V> {
    fn from_iter<I: IntoIterator<Item = (Hashed<K>, V)>>(iter: I) -> Self {
        Self::from(MiniMutInsertionMap::from_iter(iter))
    }
}

impl<'a, K, V> IntoIterator for &'a MiniFixedSortedMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    #[inline]
    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut MiniFixedSortedMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    #[inline]
    fn into_iter(self) -> IterMut<'a, K, V> {
        self.iter_mut()
    }
}

impl<K, V> IntoIterator for MiniFixedSortedMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    #[inline]
    fn into_iter(self) -> IntoIter<K, V> {
        self.0.into_iter()
    }
}

impl<K: Serialize, V: Serialize> Serialize for MiniFixedSortedMap<K, V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, K, V> Deserialize<'de> for MiniFixedSortedMap<K, V>
where
    K: Deserialize<'de> + Hash + Ord,
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        MiniMutInsertionMap::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(feature = "pagable_dep")]
impl<K: Pagable, V: Pagable> PagableSerialize for MiniFixedSortedMap<K, V> {
    fn pagable_serialize(
        &self,
        serializer: &mut dyn pagable::PagableSerializer,
    ) -> pagable::Result<()> {
        self.0.pagable_serialize(serializer)
    }
}

#[cfg(feature = "pagable_dep")]
impl<'de, K: Pagable + Ord, V: Pagable> PagableDeserialize<'de> for MiniFixedSortedMap<K, V> {
    fn pagable_deserialize<D: pagable::PagableDeserializer<'de> + ?Sized>(
        deserializer: &mut D,
    ) -> pagable::Result<Self> {
        // Serialized by this type, so already sorted; sorting again is a cheap way not to trust
        // bytes with an invariant.
        Ok(Self::from(MiniMutInsertionMap::pagable_deserialize(
            deserializer,
        )?))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use super::*;

    /// A deterministic shuffle of `0..n`.
    fn shuffled(n: u64, seed: u64) -> Vec<u64> {
        let mut keys: Vec<u64> = (0..n).collect();
        let mut state = seed | 1;
        for i in (1..keys.len()).rev() {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            keys.swap(i, (state >> 33) as usize % (i + 1));
        }
        keys
    }

    fn hash_of<T: Hash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    fn strong_hash_of<T: StrongHash>(value: &T) -> u64 {
        let mut hasher = DefaultHasher::new();
        value.strong_hash(&mut hasher);
        hasher.finish()
    }

    /// Sizes on both sides of the index threshold.
    const SIZES: &[u64] = &[0, 1, 2, 3, 16, 32, 33, 64, 100, 1000];

    /// Writing values leaves the keys, their ranks and the canonical order alone; consuming the
    /// map yields the entries in key order.
    #[test]
    fn values_are_writable_and_entries_move_out_in_key_order() {
        for &n in SIZES {
            let mut map: MiniFixedSortedMap<u64, u64> =
                shuffled(n, 11).into_iter().map(|k| (k, 0)).collect();
            for (k, v) in map.iter_mut() {
                *v = *k;
            }
            for v in map.values_mut() {
                *v *= 2;
            }
            for (k, v) in &mut map {
                *v += *k;
            }
            for k in 0..n {
                *map.get_mut(&k).unwrap() += 1;
            }
            assert_eq!(None, map.get_mut(&n));
            map.assert_invariants();
            let expected: MiniFixedSortedMap<u64, u64> = (0..n).map(|k| (k, k * 3 + 1)).collect();
            assert_eq!(expected, map);
            assert!(map.clone().into_keys().eq(0..n));
            assert!(map.clone().into_values().eq((0..n).map(|k| k * 3 + 1)));
            assert!(map.into_iter().eq((0..n).map(|k| (k, k * 3 + 1))));
        }
    }

    #[test]
    fn sorted_and_canonical_whatever_the_arrival_order() {
        for &n in SIZES {
            let a: MiniFixedSortedMap<u64, u64> =
                shuffled(n, 1).into_iter().map(|k| (k, k * 3)).collect();
            let b: MiniFixedSortedMap<u64, u64> =
                shuffled(n, 2).into_iter().map(|k| (k, k * 3)).collect();
            a.assert_invariants();
            assert_eq!(n as usize, a.len());
            assert_eq!(
                (0..n).map(|k| (k, k * 3)).collect::<Vec<_>>(),
                a.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
            );
            assert_eq!(a, b, "n={n}");
            assert_eq!(hash_of(&a), hash_of(&b), "n={n}");
            assert_eq!(strong_hash_of(&a), strong_hash_of(&b), "n={n}");
            assert_eq!(format!("{a:?}"), format!("{b:?}"), "n={n}");
            if n > 0 {
                assert_eq!(Some((&0, &0)), a.first());
                assert_eq!(Some((&(n - 1), &((n - 1) * 3))), a.last());
            }
        }
    }

    #[test]
    fn lookups_and_ranks_across_both_regimes() {
        for &n in SIZES {
            let map: MiniFixedSortedMap<u64, u64> =
                shuffled(n, 3).into_iter().map(|k| (k, k + 1)).collect();
            for k in 0..n {
                assert_eq!(Some(&(k + 1)), map.get(&k), "n={n} k={k}");
                assert!(map.contains_key(&k));
                assert_eq!(Some(k as usize), map.get_index_of(&k));
                assert_eq!(Some((&k, &(k + 1))), map.get_index(k as usize));
                assert_eq!(Some((k as usize, &k, &(k + 1))), map.get_full(&k));
                let hashed = Hashed::new(&k);
                assert_eq!(Some(&(k + 1)), map.get_hashed(hashed));
                assert!(map.contains_key_hashed(hashed));
                assert_eq!(Some(k as usize), map.get_index_of_hashed(hashed));
            }
            assert_eq!(None, map.get(&n));
            assert!(!map.contains_key(&n));
            assert_eq!(None, map.get_index(n as usize));
        }
    }

    #[test]
    fn a_repeated_key_keeps_its_last_value() {
        let map: MiniFixedSortedMap<&str, u32> = [("b", 1), ("a", 2), ("b", 3), ("c", 4), ("a", 5)]
            .into_iter()
            .collect();
        map.assert_invariants();
        assert_eq!(
            vec![("a", 5), ("b", 3), ("c", 4)],
            map.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn built_from_a_mutable_map_and_from_hashed_entries() {
        let mut mutable = MiniMutInsertionMap::new();
        for k in shuffled(50, 4) {
            mutable.insert(k.to_string(), k);
        }
        let from_mutable = MiniFixedSortedMap::from(mutable);
        from_mutable.assert_invariants();
        let from_hashed: MiniFixedSortedMap<String, u64> = shuffled(50, 5)
            .into_iter()
            .map(|k| (Hashed::new(k.to_string()), k))
            .collect();
        assert_eq!(from_mutable, from_hashed);
        // String order, not numeric.
        assert_eq!(Some((&"0".to_owned(), &0)), from_mutable.first());
        assert_eq!(Some((&"9".to_owned(), &9)), from_mutable.last());
        let back = from_hashed.into_insertion_map();
        assert_eq!(50, back.len());
        assert_eq!(Some(&0), back.get("0"));
    }

    #[test]
    fn ordering_is_lexicographic_over_entries() {
        let map = |entries: &[(u64, u64)]| -> MiniFixedSortedMap<u64, u64> {
            entries.iter().copied().collect()
        };
        assert!(map(&[]) < map(&[(1, 1)]));
        assert!(map(&[(1, 1)]) < map(&[(1, 2)]));
        assert!(map(&[(1, 2)]) < map(&[(2, 0)]));
        assert!(map(&[(1, 1)]) < map(&[(1, 1), (2, 2)]));
        assert_eq!(
            Ordering::Equal,
            map(&[(2, 2), (1, 1)]).cmp(&map(&[(1, 1), (2, 2)]))
        );
    }

    #[test]
    fn keys_and_values_follow_key_order() {
        let map: MiniFixedSortedMap<u64, &str> =
            [(3, "three"), (1, "one"), (2, "two")].into_iter().collect();
        assert_eq!(vec![&1, &2, &3], map.keys().collect::<Vec<_>>());
        assert_eq!(
            vec![&"one", &"two", &"three"],
            map.values().collect::<Vec<_>>()
        );
        assert_eq!(3, (&map).into_iter().count());
    }

    #[test]
    fn serde_round_trips_in_key_order() {
        let map: MiniFixedSortedMap<String, u64> = shuffled(40, 6)
            .into_iter()
            .map(|k| (format!("k{k:02}"), k))
            .collect();
        let json = serde_json::to_string(&map).unwrap();
        assert!(json.starts_with(r#"{"k00":0,"k01":1,"#), "{json}");
        let back: MiniFixedSortedMap<String, u64> = serde_json::from_str(&json).unwrap();
        assert_eq!(map, back);
        // Arrival order in the input does not reach the map.
        let back: MiniFixedSortedMap<String, u64> =
            serde_json::from_str(r#"{"b":2,"a":1}"#).unwrap();
        assert_eq!(
            vec![&"a".to_owned(), &"b".to_owned()],
            back.keys().collect::<Vec<_>>()
        );
    }

    #[cfg(feature = "pagable_dep")]
    #[test]
    fn pagable_round_trips() {
        let map: MiniFixedSortedMap<u64, u64> =
            shuffled(100, 7).into_iter().map(|k| (k, k * 5)).collect();
        let mut serializer = pagable::testing::TestingSerializer::new();
        map.pagable_serialize(&mut serializer).unwrap();
        let bytes = serializer.finish();
        let mut deserializer = pagable::testing::TestingDeserializer::new(&bytes);
        let back = MiniFixedSortedMap::<u64, u64>::pagable_deserialize(&mut deserializer).unwrap();
        assert_eq!(map, back);
    }

    #[test]
    fn clone_is_deep_and_equal() {
        let map: MiniFixedSortedMap<String, Vec<u8>> = (0..40u8)
            .rev()
            .map(|i| (i.to_string(), vec![i; 3]))
            .collect();
        let clone = map.clone();
        assert_eq!(map, clone);
        drop(map);
        assert_eq!(Some(&vec![7u8; 3]), clone.get("7"));
    }

    #[test]
    fn auto_traits_follow_the_entries() {
        fn send_and_sync<T: Send + Sync>() {}
        send_and_sync::<MiniFixedSortedMap<String, Vec<u8>>>();
    }
}
