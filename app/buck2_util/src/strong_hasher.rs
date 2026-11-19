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

use allocative::Allocative;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

#[derive(Default)]
pub struct Blake3StrongHasher(blake3::Hasher);

impl Blake3StrongHasher {
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    pub fn finalize(&self) -> blake3::Hash {
        self.0.finalize()
    }

    pub fn finalize128(&self) -> StrongHash128 {
        StrongHash128(
            self.0.finalize().as_bytes()[..16]
                .try_into()
                .expect("blake3 output should be 32 bytes"),
        )
    }
}

/// 128-bit truncation of a strong hash. Stored as bytes (alignment 1) so that embedding it in a
/// struct adds no padding, unlike a `u128` field which is 16-byte aligned.
#[derive(Copy, Clone, Dupe, Debug, Eq, PartialEq, Allocative, Pagable)]
pub struct StrongHash128([u8; 16]);

impl Hash for StrongHash128 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u128(u128::from_le_bytes(self.0));
    }
}

impl StrongHash for StrongHash128 {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        state.write(&self.0);
    }
}

// Other than for `usize`/`isize`, this `Hasher` impl only provides `write`
// and `finish` (not the full set of `write_*` forwarding methods). That is
// acceptable here because blake3 is a streaming hash that processes all data
// uniformly through `update`. See the comment on
// `StarlarkHasherSmallPromote` in
// `buck2_build_api/.../provider/callable.rs` for when full forwarding is needed.
impl Hasher for Blake3StrongHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    // The default implementations hash these at their native width, which
    // would make the hash platform dependent.
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64)
    }

    fn write_isize(&mut self, i: isize) {
        self.write_i64(i as i64)
    }

    fn finish(&self) -> u64 {
        let bytes = self.0.finalize().as_bytes()[..8]
            .try_into()
            .expect("Internal error: hash should be 64 bits");
        u64::from_be_bytes(bytes)
    }
}
