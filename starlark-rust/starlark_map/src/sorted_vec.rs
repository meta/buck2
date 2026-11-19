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

//! `Vec` with elements sorted.

use std::hash::Hash;
use std::slice;
use std::vec;

use allocative::Allocative;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;

/// Type which enfoces that its elements are sorted. That's it.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Allocative, Default, Serialize
)]
#[cfg_attr(feature = "pagable_dep", derive(Pagable))]
pub struct SortedVec<T> {
    vec: Vec<T>,
}

impl<'de, T> Deserialize<'de> for SortedVec<T>
where
    T: Deserialize<'de> + Ord,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        /// The wire shape of the derived `Serialize`; the input need not be sorted. Only serde
        /// input is sorted here; the derived `Pagable` impl reads `vec` directly and never
        /// goes through this impl.
        #[derive(Deserialize)]
        #[serde(rename = "SortedVec")]
        struct Repr<T> {
            vec: Vec<T>,
        }
        let Repr { vec } = Repr::<T>::deserialize(deserializer)?;
        Ok(SortedVec::from(vec))
    }
}

impl<T> SortedVec<T> {
    /// Construct an empty `SortedVec`.
    #[inline]
    pub const fn new() -> SortedVec<T> {
        SortedVec { vec: Vec::new() }
    }

    /// Construct without checking that the elements are sorted.
    #[inline]
    pub fn new_unchecked(vec: Vec<T>) -> SortedVec<T>
    where
        T: Ord,
    {
        debug_assert!(vec.iter().zip(vec.iter().skip(1)).all(|(a, b)| a <= b));
        SortedVec { vec }
    }

    /// Iterate over the elements.
    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, T> {
        self.vec.iter()
    }
}

impl<T: Ord> From<Vec<T>> for SortedVec<T> {
    #[inline]
    fn from(mut vec: Vec<T>) -> Self {
        vec.sort();
        SortedVec { vec }
    }
}

impl<T: Ord> FromIterator<T> for SortedVec<T> {
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let vec = Vec::from_iter(iter);
        SortedVec::from(vec)
    }
}

impl<T> IntoIterator for SortedVec<T> {
    type Item = T;
    type IntoIter = vec::IntoIter<T>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.vec.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use crate::sorted_vec::SortedVec;

    /// Test `new_unchecked` panics in debug mode when the elements are not sorted.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic]
    fn test_new_unchecked() {
        SortedVec::new_unchecked(vec![1, 3, 2]);
    }

    #[test]
    fn test_deserialize_sorts() {
        assert_eq!(
            serde_json::json!({"vec": [1, 2, 3]}),
            serde_json::to_value(SortedVec::from(vec![3u32, 1, 2])).unwrap()
        );
        let v: SortedVec<u32> = serde_json::from_str(r#"{"vec": [3, 1, 2]}"#).unwrap();
        assert_eq!(vec![1, 2, 3], v.iter().copied().collect::<Vec<_>>());
    }
}
