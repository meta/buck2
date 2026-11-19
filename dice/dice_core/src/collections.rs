/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Key-indexed hash collections.

use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::BuildHasher;
use std::hash::Hasher;

use crate::ids::Key;

/// A hash map keyed by [`Key`], with a hasher suited to dense small integers.
pub type KeyMap<V> = HashMap<Key, V, BuildKeyHasher>;
/// A hash set of [`Key`]s, with a hasher suited to dense small integers.
pub type KeySet = HashSet<Key, BuildKeyHasher>;

/// Hashes a [`Key`] by multiplying its index by a large odd constant: keys are dense small
/// integers, so this spreads them well at no cost, and the maps are on the hot path of every
/// operation.
#[derive(Default, Clone, Copy, Debug)]
pub struct BuildKeyHasher;

impl BuildHasher for BuildKeyHasher {
    type Hasher = KeyHasher;

    fn build_hasher(&self) -> KeyHasher {
        KeyHasher(0)
    }
}

#[derive(Default)]
pub struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u32(*b as u32);
        }
    }

    fn write_u32(&mut self, i: u32) {
        self.0 = (self.0 ^ (i as u64)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}
