/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! A port of `rustc_hash::FxHasher` (version 2, a polynomial hash designed by
//! Orson Peters) with platform-independent output: the state is pinned to `u64`
//! where upstream uses the pointer width, and byte mixing is pinned to the
//! 128-bit widening multiply where upstream substitutes a different-valued mix
//! on architectures without fast wide multiplication. On 64-bit platforms the
//! output is bit-for-bit identical to `rustc_hash::FxHasher` (enforced by test).
//!
//! Ported rather than depended on because upstream reserves the right to change its algorithm
//! between versions, and these hashes are stored: a change has to be a decision made here, which
//! the drift test turns it into.
//!
//! Ported from `rustc-hash` 2.1.3, dual-licensed MIT OR Apache-2.0:
//! <https://github.com/rust-lang/rustc-hash>

use std::hash::Hasher;

/// A multiplier found to be good for a multiplicative congruential
/// pseudorandom number generator; see the upstream sources for provenance.
const K: u64 = 0xf1357aea2e62a9c5;

// Nothing special, digits of pi.
const SEED1: u64 = 0x243f6a8885a308d3;
const SEED2: u64 = 0x13198a2e03707344;
const PREVENT_TRIVIAL_ZERO_COLLAPSE: u64 = 0xa4093822299f31d0;

#[inline]
fn multiply_mix(x: u64, y: u64) -> u64 {
    // The middle bits of the full 128-bit product fluctuate the most with small
    // changes in the input: the top bits of `lo` and the bottom bits of `hi`,
    // so XOR the halves. Unlike upstream, use this path on every architecture:
    // compilers lower `u128` multiplication everywhere, and determinism matters
    // more than speed on the 32-bit targets that would prefer a cheaper mix.
    let full = (x as u128).wrapping_mul(y as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

/// A wyhash-inspired non-collision-resistant hash for strings/slices designed
/// by Orson Peters, with a focus on small strings and small codesize.
///
/// No avalanching here: the result feeds a multiplication after which the high
/// bits are taken, which avalanches for us.
#[inline]
fn hash_bytes(bytes: &[u8]) -> u64 {
    let len = bytes.len();
    let mut s0 = SEED1;
    let mut s1 = SEED2;

    if len <= 16 {
        // XOR the input into s0, s1.
        if len >= 8 {
            s0 ^= u64::from_le_bytes(bytes[0..8].try_into().unwrap());
            s1 ^= u64::from_le_bytes(bytes[len - 8..].try_into().unwrap());
        } else if len >= 4 {
            s0 ^= u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as u64;
            s1 ^= u32::from_le_bytes(bytes[len - 4..].try_into().unwrap()) as u64;
        } else if len > 0 {
            let lo = bytes[0];
            let mid = bytes[len / 2];
            let hi = bytes[len - 1];
            s0 ^= lo as u64;
            s1 ^= ((hi as u64) << 8) | mid as u64;
        }
    } else {
        // Handle bulk (can partially overlap with suffix).
        let mut bulk = &bytes[..(len - 1)];
        while let Some((chunk, rest)) = bulk.split_first_chunk::<16>() {
            let x = u64::from_le_bytes((&chunk[..8]).try_into().unwrap());
            let y = u64::from_le_bytes((&chunk[8..]).try_into().unwrap());

            // Replace s1 with a mix of s0, x, and y, and s0 with s1. This
            // ensures the compiler can unroll this loop into two independent
            // streams, one operating on s0, the other on s1.
            //
            // Since zeroes are a common input we prevent an immediate trivial
            // collapse of the hash function by XOR'ing a constant with y.
            let t = multiply_mix(s0 ^ x, PREVENT_TRIVIAL_ZERO_COLLAPSE ^ y);
            s0 = s1;
            s1 = t;
            bulk = rest;
        }

        let suffix = &bytes[len - 16..];
        s0 ^= u64::from_le_bytes(suffix[0..8].try_into().unwrap());
        s1 ^= u64::from_le_bytes(suffix[8..16].try_into().unwrap());
    }

    multiply_mix(s0, s1) ^ (len as u64)
}

#[derive(Default, Clone)]
pub(crate) struct Fx64Hasher {
    hash: u64,
}

impl Fx64Hasher {
    #[inline]
    fn add_to_hash(&mut self, i: u64) {
        self.hash = self.hash.wrapping_add(i).wrapping_mul(K);
    }
}

impl Hasher for Fx64Hasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Compress the byte string to a single u64 and add to our hash.
        self.write_u64(hash_bytes(bytes));
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.add_to_hash(i as u64);
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.add_to_hash(i as u64);
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.add_to_hash(i as u64);
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.add_to_hash(i);
    }

    #[inline]
    fn write_u128(&mut self, i: u128) {
        self.add_to_hash(i as u64);
        self.add_to_hash((i >> 64) as u64);
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.add_to_hash(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        // A multiplicative hash concentrates entropy in the top bits, while hash
        // table implementations take bucket indexes from the bottom bits, so
        // rotate the good bits down (upstream's 64-bit rotation amount).
        self.hash.rotate_left(26)
    }
}

#[cfg(test)]
mod tests {
    use std::hash::Hash;
    use std::hash::Hasher;

    use super::*;

    fn test_inputs() -> Vec<Vec<u8>> {
        let mut inputs = Vec::new();
        for len in 0..=64 {
            inputs.push((0..len).map(|i| i as u8).collect());
            inputs.push(vec![0; len]);
            inputs.push(
                (0..len)
                    .map(|i| (i as u8).wrapping_mul(37) ^ 0x5a)
                    .collect(),
            );
        }
        inputs
    }

    const UPSTREAM_DRIFT: &str = "\
upstream `rustc-hash` has changed its hashing algorithm relative to the version this \
module was ported from (2.1.3). That is allowed upstream (hash stability across \
versions is an explicit non-goal there) and is exactly why the algorithm is vendored \
here. Do NOT update this port to silently match. If the new algorithm is worth \
adopting, treat it as a deliberate change to `ShortHash` for every consumer of this \
crate, and update the golden values in `stable_across_platforms` with it. If the new \
algorithm is not worth adopting, this test may instead pin/skip the new `rustc-hash` \
test dependency version.";

    /// Upstream takes this exact code path on 64-bit architectures with fast
    /// widening multiplication, so the port must match it bit for bit there.
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn matches_rustc_hash() {
        for input in test_inputs() {
            let mut ours = Fx64Hasher::default();
            let mut theirs = rustc_hash::FxHasher::default();
            ours.write(&input);
            theirs.write(&input);
            assert_eq!(
                ours.finish(),
                theirs.finish(),
                "for bytes {input:?}: {UPSTREAM_DRIFT}"
            );
        }

        fn feed(mut hasher: &mut dyn Hasher) {
            hasher.write_u8(1);
            hasher.write_u16(2);
            hasher.write_u32(3);
            hasher.write_u64(4);
            hasher.write_u128(5 + (6u128 << 64));
            hasher.write_usize(7);
            "starlark".hash(&mut hasher);
        }
        let mut ours = Fx64Hasher::default();
        let mut theirs = rustc_hash::FxHasher::default();
        feed(&mut ours);
        feed(&mut theirs);
        assert_eq!(ours.finish(), theirs.finish(), "{UPSTREAM_DRIFT}");
    }

    /// Golden values locking the platform-independent output; these must hold
    /// on every architecture, pointer width, and endianness.
    #[test]
    fn stable_across_platforms() {
        fn hash_str(s: &str) -> u64 {
            let mut hasher = Fx64Hasher::default();
            s.hash(&mut hasher);
            hasher.finish()
        }

        assert_eq!(
            [
                hash_str(""),
                hash_str("hello"),
                hash_str("fbcode//some/package/path:some_rule_name_12345"),
            ],
            [
                13933120620573868840,
                12393608695761977456,
                17686632965210403629,
            ],
            "the hash function changed; this must only ever happen deliberately"
        );
    }
}
