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
use std::hash::Hash;
use std::ops::Deref;

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;
use triomphe::Arc;

use crate::exact_size_iter::IterExactSize;

/// `Arc<[T]>` but more efficient.
#[derive(Debug, Allocative, Pagable)]
pub struct ArcSlice<T> {
    // This can be `NonNull<[T]>` when `Arc::from_raw` is available:
    // https://github.com/Manishearth/triomphe/pull/57/files
    slice: Option<Arc<[T]>>,
}

impl<T: Serialize> Serialize for ArcSlice<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.deref().serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ArcSlice<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v: Vec<_> = Vec::deserialize(deserializer)?;
        Ok(v.into())
    }
}

impl<T> ArcSlice<T> {
    #[inline]
    pub fn new<const N: usize>(array: [T; N]) -> ArcSlice<T> {
        ArcSlice::from_iter(array)
    }
}

impl<T> Clone for ArcSlice<T> {
    #[inline]
    fn clone(&self) -> ArcSlice<T> {
        ArcSlice {
            slice: self.slice.clone(),
        }
    }
}

impl<T> Dupe for ArcSlice<T> {}

impl<T> Deref for ArcSlice<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        match self.slice {
            Some(ref slice) => slice,
            None => &[],
        }
    }
}

impl<T> Default for ArcSlice<T> {
    #[inline]
    fn default() -> ArcSlice<T> {
        ArcSlice { slice: None }
    }
}

impl<T: Eq> PartialEq for ArcSlice<T> {
    #[inline]
    fn eq(&self, other: &ArcSlice<T>) -> bool {
        if let (Some(this), Some(other)) = (&self.slice, &other.slice) {
            // We require `T: Eq` so we can use `Arc::ptr_eq`.
            if Arc::ptr_eq(this, other) {
                return true;
            }
        }
        self[..] == other[..]
    }
}

impl<T: Eq> Eq for ArcSlice<T> {}

impl<T: Hash> Hash for ArcSlice<T> {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // This must hash as slice because we implement `Borrow<[T]>`.
        self[..].hash(state)
    }
}

impl<T: StrongHash> StrongHash for ArcSlice<T> {
    #[inline]
    fn strong_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // This must hash as slice because we implement `Borrow<[T]>`.
        self[..].strong_hash(state)
    }
}

impl<T: PartialOrd + std::cmp::Eq> PartialOrd for ArcSlice<T> {
    fn partial_cmp(&self, other: &ArcSlice<T>) -> Option<std::cmp::Ordering> {
        self[..].partial_cmp(&other[..])
    }
}

impl<T: Ord> Ord for ArcSlice<T> {
    fn cmp(&self, other: &ArcSlice<T>) -> std::cmp::Ordering {
        self[..].cmp(&other[..])
    }
}

impl<T> Borrow<[T]> for ArcSlice<T> {
    #[inline]
    fn borrow(&self) -> &[T] {
        self
    }
}

impl<T> FromIterator<T> for ArcSlice<T> {
    #[inline]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        if upper == Some(0) {
            ArcSlice::default()
        } else if Some(lower) == upper {
            let arc = Arc::from_header_and_iter((), iter.with_exact_size(lower));
            let arc: Arc<[T]> = arc.into();
            if arc.is_empty() {
                ArcSlice::default()
            } else {
                ArcSlice { slice: Some(arc) }
            }
        } else {
            let vec = Vec::from_iter(iter);
            ArcSlice::from(vec)
        }
    }
}

impl<T> From<Vec<T>> for ArcSlice<T> {
    #[inline]
    fn from(vec: Vec<T>) -> ArcSlice<T> {
        if vec.is_empty() {
            ArcSlice::default()
        } else {
            ArcSlice {
                slice: Some(Arc::from(vec)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::arc_str::slice::ArcSlice;

    #[test]
    fn test_from_iter_exact_size() {
        let slice = ArcSlice::from_iter([17, 19]);
        assert_eq!([17, 19], *slice);
        let slice = ArcSlice::from_iter([21; 0]);
        assert_eq!([23; 0], *slice);
    }

    #[test]
    fn test_from_iter_unknown_size() {
        let slice = ArcSlice::from_iter(vec![17, 19].into_iter().filter(|_| true));
        assert_eq!([17, 19], *slice);
        let slice = ArcSlice::from_iter(vec![17, 19].into_iter().filter(|_| false));
        assert_eq!([23; 0], *slice);
    }
}
