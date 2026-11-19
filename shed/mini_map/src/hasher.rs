/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::BuildHasher;
use std::hash::Hasher;

use allocative::Allocative;
use dupe::Dupe;
#[cfg(feature = "pagable_dep")]
use pagable::Pagable;

use crate::fx64::Fx64Hasher;
use crate::short_hash::ShortHash;

/// The hasher used by this crate's maps, and the one buck2 and starlark hash with.
///
/// Optimized for speed, not for resistance to hash flooding, so it must not be used on
/// attacker-controlled keys. Its output is identical on every platform, regardless of endianness
/// and pointer width, but is not stable across versions of this crate.
///
/// # Wrapping this hasher
///
/// A [`Hasher`] that wraps another one must forward every `write_*` method explicitly. This is not
/// a correctness question - both paths produce valid hashes - but a performance one: the default
/// [`Hasher`] methods serialize the value to bytes and call [`Hasher::write`], whereas this hasher
/// consumes a typed write directly as a word. A forwarding method that is left out does not fail
/// to compile, it just silently takes the slow path.
#[derive(Default)]
pub struct FastHasher(Fx64Hasher);

impl FastHasher {
    /// Creates a new hasher.
    #[inline]
    pub fn new() -> FastHasher {
        FastHasher::default()
    }

    /// Finish the hash computation, folded down to a [`ShortHash`].
    #[inline]
    pub fn finish_short(&self) -> ShortHash {
        // Fold the halves together rather than truncating: multiply-based hashers
        // drive their entropy toward the high bits, and plain low-32 truncation made
        // names differing only in trailing bytes collide deterministically.
        let hash = self.finish();
        ShortHash::new_unchecked((hash ^ (hash >> 32)) as u32)
    }
}

// IMPORTANT: `Hasher` wrappers must explicitly forward every `write_*` method
// to the inner hasher. See the doc comment on `FastHasher` for details.
#[allow(clippy::missing_trait_methods)]
impl Hasher for FastHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish()
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes)
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.0.write_u8(i)
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.0.write_u16(i)
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.0.write_u32(i)
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0.write_u64(i)
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.0.write_u128(i)
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.0.write_usize(i)
    }
}

/// [`BuildHasher`] implementation which produces [`FastHasher`].
#[derive(Default, Debug, Clone, Copy, Dupe, Allocative)]
#[cfg_attr(feature = "pagable_dep", derive(Pagable))]
pub struct FastHasherBuilder;

impl BuildHasher for FastHasherBuilder {
    type Hasher = FastHasher;

    #[inline]
    fn build_hasher(&self) -> FastHasher {
        FastHasher::new()
    }
}

#[cfg(test)]
mod tests {
    use std::hash::Hash;

    use super::*;

    fn hash<T: Hash>(value: &T) -> u64 {
        let mut hasher = FastHasher::new();
        value.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn deterministic() {
        assert_eq!(hash(&42u64), hash(&42u64));
        assert_ne!(hash(&42u64), hash(&43u64));
    }

    /// A missing `write_*` forwarding method still produces valid hashes, so the only thing that
    /// catches one is feeding the same typed writes to the inner hasher and comparing.
    #[test]
    fn multi_write_sequence() {
        fn feed(hasher: &mut FastHasher) {
            hasher.write_u8(1);
            hasher.write_u16(2);
            hasher.write_u32(3);
            hasher.write_u64(4);
            hasher.write_u128(5);
            hasher.write_usize(6);
            hasher.write(b"seven");
        }

        let mut wrapper = FastHasher::new();
        feed(&mut wrapper);

        let mut inner = Fx64Hasher::default();
        inner.write_u8(1);
        inner.write_u16(2);
        inner.write_u32(3);
        inner.write_u64(4);
        inner.write_u128(5);
        inner.write_usize(6);
        inner.write(b"seven");

        assert_eq!(
            wrapper.finish(),
            inner.finish(),
            "`FastHasher` must forward every `write_*` method to the inner hasher"
        );
    }

    #[test]
    fn build_hasher() {
        assert_eq!(
            FastHasherBuilder.hash_one("hello"),
            FastHasherBuilder.hash_one("hello")
        );
    }
}
