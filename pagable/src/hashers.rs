/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Hashers for maps whose keys are already well-distributed hashes.

use std::any::TypeId;
use std::hash::BuildHasherDefault;
use std::hash::Hasher;

use dashmap::DashMap;

/// A concurrent map specialized for `TypeId` keys.
///
/// `TypeId` is already hash-derived, so `TypeIdHasher` avoids redundant
/// mixing. Fixing the key type in this alias also prevents the pass-through
/// hasher from being paired with arbitrarily distributed keys.
pub(crate) type TypeIdDashMap<V> = DashMap<TypeId, V, BuildHasherDefault<TypeIdHasher>>;

/// Passes the already-uniform `TypeId` bits through instead of re-hashing
/// them: the key is a compiler-generated hash, so further mixing is
/// redundant (std's own `Hash for TypeId` makes the same argument when it
/// feeds only half the id).
#[derive(Default)]
pub(crate) struct TypeIdHasher(u64);

impl Hasher for TypeIdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    // Fallback for `Hash` impls that feed raw bytes; `TypeId`'s bits are
    // hash-derived already, so byte folding is enough.
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = self.0.rotate_left(8) ^ u64::from(byte);
        }
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }

    fn write_u128(&mut self, n: u128) {
        self.0 = n as u64;
    }
}
