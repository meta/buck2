/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Linear scans over an array of stored hashes, vectorized on nightly.

/// Index of the first element of `hashes` equal to `hash`.
#[inline]
pub(crate) fn find_hash(hashes: &[u32], hash: u32) -> Option<usize> {
    #[cfg(rust_nightly)]
    {
        simd::find_hash(hashes, hash)
    }
    #[cfg(not(rust_nightly))]
    {
        scalar_find_hash(hashes, hash)
    }
}

#[inline(always)]
fn scalar_find_hash(hashes: &[u32], hash: u32) -> Option<usize> {
    let mut i = 0;
    while i < hashes.len() {
        if hashes[i] == hash {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(rust_nightly)]
mod simd {
    use std::simd::Simd;
    use std::simd::cmp::SimdPartialEq;

    /// 128-bit vectors: available on x86_64 and aarch64, and a better fit than wider ones for the
    /// short runs this scans.
    type T = Simd<u32, 4>;
    const LANES: usize = T::LEN;

    #[inline]
    pub(super) fn find_hash(hashes: &[u32], hash: u32) -> Option<usize> {
        if hashes.len() < LANES {
            return super::scalar_find_hash(hashes, hash);
        }
        let needle = T::splat(hash);
        let mut i = 0;
        // Whole vectors, then one more vector ending at the last element, which may overlap the
        // previous one: a repeated element is still the first match in index order.
        while i + LANES < hashes.len() {
            // SAFETY: the loop condition leaves more than `LANES` elements from `i`.
            let chunk = T::from_slice(unsafe { hashes.get_unchecked(i..i + LANES) });
            let eq = chunk.simd_eq(needle);
            if eq.any() {
                return Some(i + eq.to_bitmask().trailing_zeros() as usize);
            }
            i += LANES;
        }
        let start = hashes.len() - LANES;
        // SAFETY: `hashes.len() >= LANES`.
        let chunk = T::from_slice(unsafe { hashes.get_unchecked(start..) });
        let eq = chunk.simd_eq(needle);
        if eq.any() {
            Some(start + eq.to_bitmask().trailing_zeros() as usize)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_hash_matches_reference() {
        assert_eq!(None, find_hash(&[], 77));
        for len in 1..20 {
            let array: Vec<u32> = (0..len).map(|_| 88).collect();
            assert_eq!(None, find_hash(&array, 77));
            for i in 0..len {
                let array: Vec<u32> = (0..len).map(|j| if j == i { 77 } else { 88 }).collect();
                assert_eq!(Some(i), find_hash(&array, 77));
            }
            for i in 0..len {
                // The first of several matches is the one returned.
                let array: Vec<u32> = (0..len).map(|j| if j < i { 88 } else { 77 }).collect();
                assert_eq!(Some(i), find_hash(&array, 77));
            }
        }
    }

    /// The SIMD paths are only compiled on nightly, so on that toolchain check them against the
    /// scalar ones directly rather than trusting that both were covered by the tests above.
    #[cfg(rust_nightly)]
    #[test]
    fn simd_agrees_with_scalar() {
        for len in 0..40usize {
            let hashes: Vec<u32> = (0..len).map(|i| i as u32 * 3).collect();
            for needle in 0..(len as u32 * 3 + 5) {
                assert_eq!(
                    scalar_find_hash(&hashes, needle),
                    simd::find_hash(&hashes, needle),
                    "len={len} needle={needle}"
                );
            }
        }
    }
}
