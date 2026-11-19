/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! A Map with deterministic iteration order that specializes its storage based on the number of
//! entries to optimize memory. This is essentially `IndexMap` with two changes:
//! * no index is created for small maps
//! * short hashes are stored next to keys

use std::fmt;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;
use std::ops::DerefMut;

use allocative::Allocative;
use mini_map::MiniMutInsertionMap;
pub use mini_map::mut_insertion_map::Entry;
pub use mini_map::mut_insertion_map::IntoIter;
pub use mini_map::mut_insertion_map::IntoIterHashed;
pub use mini_map::mut_insertion_map::IntoKeys;
pub use mini_map::mut_insertion_map::IntoValues;
pub use mini_map::mut_insertion_map::Iter;
pub use mini_map::mut_insertion_map::IterHashed;
pub use mini_map::mut_insertion_map::IterMut;
pub use mini_map::mut_insertion_map::IterMutUnchecked;
pub use mini_map::mut_insertion_map::Keys;
pub use mini_map::mut_insertion_map::OccupiedEntry;
pub use mini_map::mut_insertion_map::VacantEntry;
pub use mini_map::mut_insertion_map::Values;
pub use mini_map::mut_insertion_map::ValuesMut;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;

use crate::Hashed;

/// A map with deterministic iteration order.
///
/// This map is similar to [`indexmap::IndexMap`](https://docs.rs/indexmap)
/// with the following differences:
/// - [Small hashes](crate::StarlarkHashValue) are stored next to keys
/// - Index is not created for small maps
///
/// The representation and the whole read/write API live in [`MiniMutInsertionMap`] and are reached
/// through [`Deref`] rather than restated here. What this type adds is starlark's historical
/// `PartialEq`, which compares contents without regard to iteration order. The shed type has no
/// `PartialEq` at all, deliberately: for a map you can still mutate, neither the ordered nor the
/// unordered answer is obviously the right one, so it makes you say which you meant.
#[derive(Clone, Allocative)]
#[cfg_attr(feature = "pagable_dep", derive(Pagable))]
#[repr(transparent)]
pub struct SmallMap<K, V>(MiniMutInsertionMap<K, V>);

impl<K, V> SmallMap<K, V> {
    /// Empty map.
    #[inline]
    pub const fn new() -> Self {
        SmallMap(MiniMutInsertionMap::new())
    }

    /// Create an empty map with specified capacity.
    #[inline]
    pub fn with_capacity(n: usize) -> Self {
        SmallMap(MiniMutInsertionMap::with_capacity(n))
    }

    /// Number of entries.
    ///
    /// Also inherent, not just reachable through [`Deref`], so that `SmallMap::len` works as a
    /// path — `#[serde(skip_serializing_if = "SmallMap::is_empty")]` needs that.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` iff the map has no entries.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrow the underlying shed map.
    #[inline]
    pub fn as_inner(&self) -> &MiniMutInsertionMap<K, V> {
        &self.0
    }

    /// Mutably borrow the underlying shed map.
    #[inline]
    pub fn as_inner_mut(&mut self) -> &mut MiniMutInsertionMap<K, V> {
        &mut self.0
    }

    /// Unwrap to the underlying shed map. Free: the two have the same representation.
    ///
    /// Also the way to freeze one, `map.into_inner().freeze()`: a `freeze` here would collide
    /// with starlark's `Freeze::freeze` on the same type, and an inherent method wins silently.
    #[inline]
    pub fn into_inner(self) -> MiniMutInsertionMap<K, V> {
        self.0
    }

    /// Key owned iterator.
    #[inline]
    pub fn into_keys(self) -> IntoKeys<K, V> {
        self.0.into_keys()
    }

    /// Value owned iterator.
    #[inline]
    pub fn into_values(self) -> IntoValues<K, V> {
        self.0.into_values()
    }

    /// Entries with hashes iterator.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<K, V> {
        self.0.into_iter_hashed()
    }

    /// Equal if the keys and values are equal in the iteration order.
    #[inline]
    pub fn eq_ordered(&self, other: &Self) -> bool
    where
        K: PartialEq,
        V: PartialEq,
    {
        self.0.eq_ordered(&other.0)
    }

    /// Equal if the same keys map to equal values, whatever the iteration order.
    ///
    /// This is what `==` does on this type.
    #[inline]
    pub fn eq_unordered(&self, other: &Self) -> bool
    where
        K: Eq,
        V: PartialEq,
    {
        self.0.eq_unordered(&other.0)
    }

    /// Hash entries in the iteration order.
    ///
    /// Note, keys are not hashed, but previously computed hashes are hashed instead.
    #[inline]
    pub fn hash_ordered<H: Hasher>(&self, state: &mut H)
    where
        K: Hash,
        V: Hash,
    {
        self.0.hash_ordered(state)
    }
}

impl<K, V> Deref for SmallMap<K, V> {
    type Target = MiniMutInsertionMap<K, V>;

    #[inline]
    fn deref(&self) -> &MiniMutInsertionMap<K, V> {
        &self.0
    }
}

impl<K, V> DerefMut for SmallMap<K, V> {
    #[inline]
    fn deref_mut(&mut self) -> &mut MiniMutInsertionMap<K, V> {
        &mut self.0
    }
}

impl<K, V> From<MiniMutInsertionMap<K, V>> for SmallMap<K, V> {
    #[inline]
    fn from(map: MiniMutInsertionMap<K, V>) -> Self {
        SmallMap(map)
    }
}

impl<K, V> From<SmallMap<K, V>> for MiniMutInsertionMap<K, V> {
    #[inline]
    fn from(map: SmallMap<K, V>) -> Self {
        map.0
    }
}

impl<K, V> Default for SmallMap<K, V> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<K: Debug, V: Debug> Debug for SmallMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

/// Compares contents without regard to iteration order, which is what starlark has always meant by
/// `==` here. [`eq_ordered`](SmallMap::eq_ordered) answers the other question.
impl<K: Eq, V: PartialEq> PartialEq for SmallMap<K, V> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_unordered(&other.0)
    }
}

impl<K: Eq, V: Eq> Eq for SmallMap<K, V> {}

impl<K, V> FromIterator<(K, V)> for SmallMap<K, V>
where
    K: Hash + Eq,
{
    #[inline]
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        SmallMap(MiniMutInsertionMap::from_iter(iter))
    }
}

impl<K, V> FromIterator<(Hashed<K>, V)> for SmallMap<K, V>
where
    K: Eq,
{
    #[inline]
    fn from_iter<I: IntoIterator<Item = (Hashed<K>, V)>>(iter: I) -> Self {
        SmallMap(MiniMutInsertionMap::from_iter(iter))
    }
}

impl<K, V> IntoIterator for SmallMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;

    #[inline]
    fn into_iter(self) -> IntoIter<K, V> {
        self.0.into_iter()
    }
}

impl<'a, K, V> IntoIterator for &'a SmallMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;

    #[inline]
    fn into_iter(self) -> Iter<'a, K, V> {
        self.0.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut SmallMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;

    #[inline]
    fn into_iter(self) -> IterMut<'a, K, V> {
        self.0.iter_mut()
    }
}

impl<K, V> Extend<(K, V)> for SmallMap<K, V>
where
    K: Hash + Eq,
{
    #[inline]
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        self.0.extend(iter)
    }
}

impl<K: Serialize, V: Serialize> Serialize for SmallMap<K, V> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, K, V> Deserialize<'de> for SmallMap<K, V>
where
    K: Deserialize<'de> + Hash + Eq,
    V: Deserialize<'de>,
{
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        MiniMutInsertionMap::deserialize(deserializer).map(SmallMap)
    }
}

/// Create a [`SmallMap`](SmallMap) from a list of key-value pairs.
///
/// ## Example
///
/// ```
/// use starlark_map::smallmap;
///
/// let map = smallmap! {
///     "a" => 1,
///     "b" => 2,
/// };
/// assert_eq!(map.get("a"), Some(&1));
/// assert_eq!(map.get("b"), Some(&2));
/// assert_eq!(map.get("c"), None);
/// ```
#[macro_export]
macro_rules! smallmap {
    (@single $($x:tt)*) => (());
    (@count $($rest:expr),*) => (<[()]>::len(&[$(smallmap!(@single $rest)),*]));

    ($($key:expr => $value:expr,)+) => { smallmap!($($key => $value),+) };
    ($($key:expr => $value:expr),*) => {
        {
            let cap = smallmap!(@count $($key),*);
            #[allow(unused_mut)]
            let mut map = $crate::small_map::SmallMap::with_capacity(cap);
            $(
                map.insert($key, $value);
            )*
            map
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The representation is tested in `mini_map`. What needs testing here is the facade, and above
    /// all the equality it exists to preserve.
    #[test]
    fn eq_ignores_iteration_order() {
        let a: SmallMap<u32, u32> = [(1, 10), (2, 20)].into_iter().collect();
        let b: SmallMap<u32, u32> = [(2, 20), (1, 10)].into_iter().collect();
        assert_eq!(a, b, "`==` must not care about insertion order");
        assert!(a.eq_unordered(&b));
        assert!(!a.eq_ordered(&b), "`eq_ordered` is the other question");
        assert!(a.eq_ordered(&a));

        let c: SmallMap<u32, u32> = [(1, 10), (2, 21)].into_iter().collect();
        assert_ne!(a, c);
    }

    #[test]
    fn api_reaches_through_to_the_shed_map() {
        let mut map = SmallMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        assert_eq!(Some(&1), map.get("a"));
        assert_eq!(2, map.len());
        assert_eq!(Some((0, &"a", &1)), map.get_full("a"));
        assert_eq!(vec![(&"a", &1), (&"b", &2)], map.iter().collect::<Vec<_>>());
        assert_eq!(Some(("a", 1)), map.shift_remove_entry("a"));
        *map.entry("b").or_insert(0) += 10;
        assert_eq!(Some(&12), map.get("b"));
    }

    #[test]
    fn converts_to_and_from_the_shed_map_for_free() {
        let map: SmallMap<u32, u32> = [(1, 10)].into_iter().collect();
        let inner: MiniMutInsertionMap<u32, u32> = map.clone().into();
        assert_eq!(Some(&10), inner.get(&1));
        let back: SmallMap<u32, u32> = inner.into();
        assert_eq!(map, back);
        assert_eq!(Some(&10), map.as_inner().get(&1));
    }

    #[test]
    fn macro_builds_a_map() {
        let map = smallmap! { "a" => 1, "b" => 2 };
        assert_eq!(Some(&1), map.get("a"));
        assert_eq!(2, map.len());
    }

    #[test]
    fn serde_round_trips() {
        let map: SmallMap<String, u32> = [("a".to_owned(), 1)].into_iter().collect();
        let json = serde_json::to_string(&map).unwrap();
        assert_eq!(r#"{"a":1}"#, json);
        assert_eq!(map, serde_json::from_str(&json).unwrap());
    }
}
