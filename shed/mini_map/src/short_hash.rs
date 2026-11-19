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

use allocative::Allocative;
use dupe::Dupe;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;

use crate::hasher::FastHasher;

/// A 32-bit hash value.
///
/// Half the width of what [`Hasher::finish`](std::hash::Hasher::finish) hands out, which is what
/// makes it cheap enough to store one alongside every entry of a map.
#[derive(
    Clone,
    Copy,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    Debug,
    Allocative,
    Serialize,
    Deserialize
)]
#[cfg_attr(feature = "pagable_dep", derive(Pagable))]
pub struct ShortHash(u32);

impl ShortHash {
    /// Hash `key` with [`FastHasher`].
    #[inline]
    pub fn new<K: Hash + ?Sized>(key: &K) -> Self {
        let mut hasher = FastHasher::new();
        key.hash(&mut hasher);
        hasher.finish_short()
    }

    /// Directly create a [`ShortHash`] from a hash.
    ///
    /// The caller is expected to pass something well swizzled; anything else will collide a lot.
    #[inline]
    pub const fn new_unchecked(hash: u32) -> Self {
        ShortHash(hash)
    }

    /// Narrow a 64-bit hash to a [`ShortHash`].
    ///
    /// The input does not need to be well swizzled; this mixes it.
    #[inline]
    pub const fn hash_64(h: u64) -> Self {
        // `fmix64` function from MurMur3 hash (which is in public domain).
        // https://github.com/aappleby/smhasher/blob/61a0530f28277f2e850bfc39600ce61d02b518de/src/MurmurHash3.cpp#L81

        let h = h ^ (h >> 33);
        let h = h.wrapping_mul(0xff51afd7ed558ccd);
        let h = h ^ (h >> 33);
        let h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
        let h = h ^ (h >> 33);

        ShortHash::new_unchecked(h as u32)
    }

    /// Get the integer hash value.
    #[inline]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// Widen back to a 64-bit hash good enough for a hashbrown table.
    #[inline]
    pub const fn promote(self) -> u64 {
        // Hashbrown, `HashMap` and `IndexMap` take the upper 7 bits of the hash as a tag and
        // compare 16 tags at a time with a SIMD instruction. Without the mix, the typical
        // low-valued u32 id would land in tag 0 along with every other one.
        (self.0 as u64).wrapping_mul(0x9e3779b97f4a7c15)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_agrees_with_new_unchecked() {
        let hash = ShortHash::new("hello");
        assert_eq!(hash, ShortHash::new_unchecked(hash.get()));
    }

    /// The fold in [`FastHasher::finish_short`] exists because low-32 truncation of a
    /// multiply-based hasher makes keys differing only in trailing bytes collide.
    #[test]
    fn trailing_bytes_do_not_collide() {
        let hashes: Vec<u32> = (0..64)
            .map(|i| ShortHash::new(&format!("fbcode//some/package/path:some_rule_name_{i}")).get())
            .collect();
        let mut deduped = hashes.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(hashes.len(), deduped.len());
    }

    #[test]
    fn hash_64_mixes() {
        let hashes: Vec<u32> = (0..64).map(|i| ShortHash::hash_64(i).get()).collect();
        let mut deduped = hashes.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert_eq!(hashes.len(), deduped.len());
    }

    /// A tag is the top 7 bits of the 64-bit hash; small ids must not all share one.
    #[test]
    fn promote_populates_the_hashbrown_tag() {
        let tags: Vec<u64> = (0..64)
            .map(|i| ShortHash::new_unchecked(i).promote() >> 57)
            .collect();
        let mut deduped = tags.clone();
        deduped.sort_unstable();
        deduped.dedup();
        assert!(deduped.len() > 1, "all of {tags:?} landed in the same tag");
    }
}
