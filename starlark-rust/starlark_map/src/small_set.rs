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

//! Ordered set.

use std::fmt;
use std::fmt::Debug;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;
use std::ops::DerefMut;

use allocative::Allocative;
use mini_map::MiniMutInsertionSet;
pub use mini_map::mut_insertion_set::Difference;
pub use mini_map::mut_insertion_set::IntoIter;
pub use mini_map::mut_insertion_set::IntoIterHashed;
pub use mini_map::mut_insertion_set::Iter;
pub use mini_map::mut_insertion_set::IterHashed;
pub use mini_map::mut_insertion_set::IterMutUnchecked;
pub use mini_map::mut_insertion_set::Union;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;

use crate::Hashed;

/// An memory-efficient set with deterministic order, based on [`SmallMap`](crate::small_map::SmallMap).
///
/// The representation and the whole read/write API live in [`MiniMutInsertionSet`] and are reached
/// through [`Deref`]. What this type adds is starlark's historical `PartialEq`, which compares
/// contents without regard to iteration order; see [`SmallMap`](crate::small_map::SmallMap) for
/// why the shed type does not have one.
#[derive(Clone, Allocative)]
#[cfg_attr(feature = "pagable_dep", derive(Pagable))]
#[repr(transparent)]
pub struct SmallSet<T>(MiniMutInsertionSet<T>);

impl<T> SmallSet<T> {
    /// Creates an empty `SmallSet`.
    #[inline]
    pub const fn new() -> Self {
        SmallSet(MiniMutInsertionSet::new())
    }

    /// Empty small set with preallocated capacity.
    #[inline]
    pub fn with_capacity(n: usize) -> Self {
        SmallSet(MiniMutInsertionSet::with_capacity(n))
    }

    /// Number of elements.
    ///
    /// Also inherent, not just reachable through [`Deref`], so that `SmallSet::len` works as a
    /// path — `#[serde(skip_serializing_if = "SmallSet::is_empty")]` needs that.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `true` iff the set has no elements.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Borrow the underlying shed set.
    #[inline]
    pub fn as_inner(&self) -> &MiniMutInsertionSet<T> {
        &self.0
    }

    /// Mutably borrow the underlying shed set.
    #[inline]
    pub fn as_inner_mut(&mut self) -> &mut MiniMutInsertionSet<T> {
        &mut self.0
    }

    /// Unwrap to the underlying shed set. Free: the two have the same representation.
    #[inline]
    pub fn into_inner(self) -> MiniMutInsertionSet<T> {
        self.0
    }

    /// Into hashed entries.
    #[inline]
    pub fn into_iter_hashed(self) -> IntoIterHashed<T> {
        self.0.into_iter_hashed()
    }
    /// Compute the difference between `self` and `other` returning a new set.
    ///
    /// Given `SmallSet::from_iter([1, 2, 3]).difference_with(&SmallSet::from_iter([2, 4, 5]))`
    /// this yields `SmallSet::from_iter([1, 3])`.
    pub fn difference_with(&self, other: &Self) -> SmallSet<T>
    where
        T: Eq + Clone,
    {
        SmallSet(self.0.difference_with(&other.0))
    }

    /// Iterate over the elements of this set not present in `other`.
    #[inline]
    pub fn difference<'a>(&'a self, other: &'a Self) -> Difference<'a, T>
    where
        T: Eq + Hash,
    {
        self.0.difference(&other.0)
    }

    /// Iterate over the elements of either set, this one first, without repeats.
    #[inline]
    pub fn union<'a>(&'a self, other: &'a Self) -> Union<'a, T>
    where
        T: Eq + Hash,
    {
        self.0.union(&other.0)
    }

    /// The elements in exactly one of the two sets: this set's, in its order, then `other`'s.
    #[inline]
    pub fn symmetric_difference(&self, other: &Self) -> SmallSet<T>
    where
        T: Eq + Clone,
    {
        SmallSet(self.0.symmetric_difference(&other.0))
    }

    /// Equal if entries are equal in iteration order.
    #[inline]
    pub fn eq_ordered(&self, other: &Self) -> bool
    where
        T: PartialEq,
    {
        self.0.eq_ordered(&other.0)
    }

    /// Equal if the sets hold equal elements, whatever the iteration order.
    ///
    /// This is what `==` does on this type.
    #[inline]
    pub fn eq_unordered(&self, other: &Self) -> bool
    where
        T: Eq,
    {
        self.0.eq_unordered(&other.0)
    }

    /// Hash entries in iteration order.
    ///
    /// Note, entries are not hashed, but previously computed hashes are hashed instead.
    #[inline]
    pub fn hash_ordered<H: Hasher>(&self, state: &mut H)
    where
        T: Hash,
    {
        self.0.hash_ordered(state)
    }

    /// Consume the set and wrap it in a [`Hashed`], pre-computing the hash
    /// from all elements in iteration order.
    #[inline]
    pub fn hashed(self) -> Hashed<Self>
    where
        T: Hash,
    {
        let hashed = self.0.hashed();
        let hash = hashed.hash();
        Hashed::new_unchecked(hash, SmallSet(hashed.into_key()))
    }
}

impl<T> Deref for SmallSet<T> {
    type Target = MiniMutInsertionSet<T>;

    #[inline]
    fn deref(&self) -> &MiniMutInsertionSet<T> {
        &self.0
    }
}

impl<T> DerefMut for SmallSet<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut MiniMutInsertionSet<T> {
        &mut self.0
    }
}

impl<T> From<MiniMutInsertionSet<T>> for SmallSet<T> {
    #[inline]
    fn from(set: MiniMutInsertionSet<T>) -> Self {
        SmallSet(set)
    }
}

impl<T> From<SmallSet<T>> for MiniMutInsertionSet<T> {
    #[inline]
    fn from(set: SmallSet<T>) -> Self {
        set.0
    }
}

impl<T> Default for SmallSet<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Debug> Debug for SmallSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

/// Compares contents without regard to iteration order, as starlark has always done here.
impl<T: Eq> PartialEq for SmallSet<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_unordered(&other.0)
    }
}

impl<T: Eq> Eq for SmallSet<T> {}

impl<T> FromIterator<T> for SmallSet<T>
where
    T: Hash + Eq,
{
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        SmallSet(MiniMutInsertionSet::from_iter(iter))
    }
}

impl<T> IntoIterator for SmallSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    #[inline]
    fn into_iter(self) -> IntoIter<T> {
        self.0.into_iter()
    }
}

impl<'a, T> IntoIterator for &'a SmallSet<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> Iter<'a, T> {
        self.0.iter()
    }
}

impl<T> Extend<T> for SmallSet<T>
where
    T: Hash + Eq,
{
    #[inline]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.0.extend(iter)
    }
}

impl<T: Serialize> Serialize for SmallSet<T> {
    #[inline]
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for SmallSet<T>
where
    T: Deserialize<'de> + Hash + Eq,
{
    #[inline]
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        MiniMutInsertionSet::deserialize(deserializer).map(SmallSet)
    }
}

/// Create a [`SmallSet`](SmallSet) from a list of values.
///
/// ## Example
///
/// ```
/// use starlark_map::smallset;
///
/// let set = smallset! {"a", "b"};
/// assert!(set.contains("a"));
/// assert_eq!(set.len(), 2);
/// assert!(!set.contains("c"));
/// ```
#[macro_export]
macro_rules! smallset {
    (@single $($x:tt)*) => (());
    (@count $($rest:expr),*) => (<[()]>::len(&[$(smallset!(@single $rest)),*]));

    ($($key:expr,)+) => { smallset!($($key),+) };
    ($($key:expr),*) => {
        {
            let cap = smallset!(@count $($key),*);
            let mut set = $crate::small_set::SmallSet::with_capacity(cap);
            $(
                set.insert($key);
            )*
            set
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_difference_is_a_small_set() {
        let a = SmallSet::from_iter([1, 2, 3]);
        let b = SmallSet::from_iter([2, 3, 4, 5]);
        let d: SmallSet<i32> = a.symmetric_difference(&b);
        assert_eq!(vec![1, 4, 5], d.iter().copied().collect::<Vec<_>>());
    }

    #[test]
    fn eq_ignores_iteration_order() {
        let a: SmallSet<u32> = [1, 2, 3].into_iter().collect();
        let b: SmallSet<u32> = [3, 2, 1].into_iter().collect();
        assert_eq!(a, b);
        assert!(a.eq_unordered(&b));
        assert!(!a.eq_ordered(&b));

        let c: SmallSet<u32> = [1, 2].into_iter().collect();
        assert_ne!(a, c);
    }

    #[test]
    fn api_reaches_through_to_the_shed_set() {
        let mut set = SmallSet::new();
        assert!(set.insert("a"));
        assert!(!set.insert("a"));
        set.insert("b");
        assert!(set.contains("a"));
        assert_eq!(2, set.len());
        assert_eq!(Some(0), set.get_index_of("a"));
        assert_eq!(vec![&"a", &"b"], set.iter().collect::<Vec<_>>());
    }

    #[test]
    fn set_operations_take_the_facade_type() {
        let a: SmallSet<u32> = [1, 2, 3].into_iter().collect();
        let b: SmallSet<u32> = [2, 4].into_iter().collect();
        assert_eq!(vec![&1, &3], a.difference(&b).collect::<Vec<_>>());
        assert_eq!(vec![&1, &2, &3, &4], a.union(&b).collect::<Vec<_>>());
    }

    #[test]
    fn hashed_wraps_the_facade_type() {
        let set: SmallSet<u32> = [1, 2].into_iter().collect();
        let hashed: Hashed<SmallSet<u32>> = set.clone().hashed();
        assert_eq!(&set, hashed.key());
    }

    #[test]
    fn macro_builds_a_set() {
        let set = smallset! { "a", "b" };
        assert!(set.contains("a"));
        assert_eq!(2, set.len());
    }
}
