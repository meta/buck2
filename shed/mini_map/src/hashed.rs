/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;
use std::hash::Hasher;
use std::ops::Deref;

use allocative::Allocative;
use dupe::Dupe;
use equivalent::Equivalent;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::short_hash::ShortHash;

/// A key and its hash.
///
/// Maps take and return these so that a key hashed once can be reused across a lookup, a
/// subsequent insertion, and a copy into another map.
#[derive(
    PartialEq,
    Eq,
    Debug,
    Clone,
    Copy,
    Dupe,
    Allocative,
    Serialize,
    Deserialize
)]
#[cfg_attr(feature = "pagable_dep", derive(Pagable))]
#[cfg_attr(feature = "pagable_dep", pagable(bound = "K: Pagable + 'static"))]
pub struct Hashed<K> {
    hash: ShortHash,
    key: K,
}

// We deliberately know that this is a hash and value, so our Eq/Hash are fine
#[allow(clippy::derived_hash_with_manual_eq)]
impl<K> Hash for Hashed<K> {
    fn hash<S: Hasher>(&self, state: &mut S) {
        // Only hash the hash, not the key.
        self.hash.hash(state)
    }
}

impl<K: StrongHash> StrongHash for Hashed<K> {
    fn strong_hash<S: Hasher>(&self, state: &mut S) {
        // Only hash the key, not the (weak) hash.
        self.key.strong_hash(state);
    }
}

impl<K> Deref for Hashed<K> {
    type Target = K;

    fn deref(&self) -> &Self::Target {
        &self.key
    }
}

impl<K: Display> Display for Hashed<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.key.fmt(f)
    }
}

impl<K: PartialOrd> PartialOrd for Hashed<K> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.key.partial_cmp(&other.key)
    }
}

impl<K: Ord> Ord for Hashed<K> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

impl<K> Hashed<K> {
    /// Create a new [`Hashed`] value using the [`Hash`] of the key.
    pub fn new(key: K) -> Self
    where
        K: Hash,
    {
        Hashed::new_unchecked(ShortHash::new(&key), key)
    }

    /// Directly create a new [`Hashed`] using a given hash value.
    /// If the hash does not correspond to the key, its will cause issues.
    pub fn new_unchecked(hash: ShortHash, key: K) -> Self {
        Hashed { hash, key }
    }

    /// Get the underlying key.
    pub fn key(&self) -> &K {
        &self.key
    }

    /// Get the underlying key, as mutable.
    pub fn key_mut(&mut self) -> &mut K {
        &mut self.key
    }

    /// Get the underlying key taking ownership.
    pub fn into_key(self) -> K {
        self.key
    }

    /// Get the underlying hash.
    pub fn hash(&self) -> ShortHash {
        self.hash
    }

    /// Convert `Hashed<K>` to `Hashed<&K>`.
    pub fn as_ref(&self) -> Hashed<&K> {
        Hashed::new_unchecked(self.hash, &self.key)
    }
}

impl<K> Hashed<&K> {
    /// Make `Hashed<K>` from `Hashed<&K>`.
    pub fn copied(self) -> Hashed<K>
    where
        K: Copy,
    {
        Hashed::new_unchecked(self.hash, *self.key)
    }

    /// Make `Hashed<K>` from `Hashed<&K>`, where `K` is `Clone`.
    pub fn cloned(self) -> Hashed<K>
    where
        K: Clone,
    {
        Hashed::new_unchecked(self.hash, self.key.clone())
    }
}

impl<K: ?Sized> Hashed<&K> {
    /// Make `Hashed<K>` from `Hashed<&K>`, where `T` is the owned form of `K`.
    pub fn owned<T>(self) -> Hashed<T>
    where
        K: Equivalent<T>,
        K: ToOwned<Owned = T>,
    {
        Hashed::new_unchecked(self.hash, self.key.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_matches_the_key() {
        let hashed = Hashed::new("hello");
        assert_eq!(hashed.hash(), ShortHash::new("hello"));
        assert_eq!(*hashed.key(), "hello");
    }

    /// Two keys that collide in 32 bits would still be unequal; equality is over the key.
    #[test]
    fn eq_is_over_the_key() {
        let same = ShortHash::new_unchecked(7);
        assert_ne!(
            Hashed::new_unchecked(same, 1),
            Hashed::new_unchecked(same, 2)
        );
    }

    #[test]
    fn borrowed_round_trip() {
        let hashed = Hashed::new("hello".to_owned());
        let borrowed: Hashed<&String> = hashed.as_ref();
        assert_eq!(borrowed.cloned(), hashed);
    }
}
