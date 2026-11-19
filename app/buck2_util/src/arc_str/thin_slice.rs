/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;
use std::hash::Hasher;
use std::mem;
use std::ops::Deref;
use std::slice;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;
use triomphe::ThinArc;

use crate::exact_size_iter::IterExactSize;

#[derive(Allocative, Debug, Pagable)]
pub struct ThinArcSlice<T> {
    slice: Option<ThinArc<(), T>>,
}

impl<T: Serialize> Serialize for ThinArcSlice<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_slice().serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ThinArcSlice<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v: Vec<T> = Vec::deserialize(deserializer)?;
        Ok(ThinArcSlice::from_iter(v))
    }
}

impl<T> Clone for ThinArcSlice<T> {
    #[inline]
    fn clone(&self) -> ThinArcSlice<T> {
        ThinArcSlice {
            slice: self.slice.clone(),
        }
    }
}

impl<T> Dupe for ThinArcSlice<T> {}

impl<T: PartialEq> PartialEq for ThinArcSlice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.as_slice() == other.as_slice()
    }
}

impl<T: Eq> Eq for ThinArcSlice<T> {}

impl<T: Hash> Hash for ThinArcSlice<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_slice().hash(state)
    }
}

impl<T: StrongHash> StrongHash for ThinArcSlice<T> {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.as_slice().strong_hash(state);
    }
}

impl<T: PartialOrd> PartialOrd for ThinArcSlice<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.as_slice().partial_cmp(other.as_slice())
    }
}

impl<T: Ord> Ord for ThinArcSlice<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_slice().cmp(other.as_slice())
    }
}

impl<T> ThinArcSlice<T> {
    #[inline]
    pub const fn empty() -> ThinArcSlice<T> {
        ThinArcSlice { slice: None }
    }

    #[inline]
    pub fn new<const N: usize>(array: [T; N]) -> ThinArcSlice<T> {
        ThinArcSlice::from_iter(array)
    }

    #[inline]
    pub fn as_slice(&self) -> &[T] {
        self
    }
}

const _: () = assert!(mem::size_of::<*const u8>() == mem::size_of::<ThinArcSlice<String>>());

impl<T> Deref for ThinArcSlice<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        match &self.slice {
            Some(slice) => &slice.slice,
            None => &[],
        }
    }
}

impl<T> Default for ThinArcSlice<T> {
    #[inline]
    fn default() -> ThinArcSlice<T> {
        ThinArcSlice { slice: None }
    }
}

impl<'a, T> IntoIterator for &'a ThinArcSlice<T> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;

    #[inline]
    fn into_iter(self) -> slice::Iter<'a, T> {
        self.iter()
    }
}

impl<T> FromIterator<T> for ThinArcSlice<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        if upper == Some(0) {
            ThinArcSlice::default()
        } else if Some(lower) == upper {
            let arc = ThinArc::from_header_and_iter((), iter.with_exact_size(lower));
            if arc.slice.is_empty() {
                ThinArcSlice::default()
            } else {
                ThinArcSlice { slice: Some(arc) }
            }
        } else {
            let vec = Vec::from_iter(iter);
            ThinArcSlice::from_iter(vec)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::arc_str::thin_slice::ThinArcSlice;

    #[test]
    fn test_empty() {
        let slice = ThinArcSlice::<String>::default();
        assert_eq!(0, slice.len());
    }

    #[test]
    fn test_new() {
        let slice = ThinArcSlice::new(["a".to_owned(), "b".to_owned()]);
        assert_eq!(["a".to_owned(), "b".to_owned()], *slice);
    }

    #[test]
    fn test_from_iter() {
        // Iterator without size hint.
        let iter = vec!["a".to_owned(), "b".to_owned()]
            .into_iter()
            .filter(|_| true);
        let slice = ThinArcSlice::from_iter(iter);
        assert_eq!(["a".to_owned(), "b".to_owned()], *slice);
    }
}
