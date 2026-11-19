/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::num::NonZeroU64;
use std::num::NonZeroU128;

use allocative::Allocative;
use dupe::Dupe;

/// A unique identifier for data stored in the paging backend.
///
/// `DataKey` is a 128-bit hash that uniquely identifies serialized data in storage.
/// It is used to retrieve data when a [`PagableArc`] is upgraded
/// after being paged out.
///
/// The 128-bit size provides an extremely low collision probability
/// (< 1e-15 after 820 billion samples).
///
/// Stored as `(NonZeroU64, u64)` rather than `u128` to keep 8-byte alignment instead of
/// 16-byte, while remaining bytemuck-compatible. The `NonZeroU64` first half lets
/// `Option<DataKey>` be niche-packed to the same size and alignment; `compute()` ensures the
/// first half is non-zero so a real key is never confused with `None`.
///
/// [`PagableArc`]: crate::PagableArc
#[derive(
    Allocative,
    Debug,
    Eq,
    PartialEq,
    Clone,
    Dupe,
    Copy,
    Hash,
    bytemuck::NoUninit
)]
#[repr(C)]
pub struct DataKey(NonZeroU64, u64);

impl DataKey {
    /// Computes a content-addressable key from serialized data and nested arcs.
    ///
    /// Uses blake3 to hash the arc count, data bytes, and nested arc keys together.
    /// Returns a DataKey with a value guaranteed to be non-zero.
    pub fn compute(arcs: usize, data: &[u8], more_data: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&arcs.to_le_bytes());
        hasher.update(data);
        hasher.update(more_data);
        let hash = hasher.finalize();

        // Use the first 128 bits of the blake3 hash
        let hash_bytes = hash.as_bytes();
        let lo = u64::from_le_bytes(hash_bytes[..8].try_into().unwrap());
        let hi = u64::from_le_bytes(hash_bytes[8..16].try_into().unwrap());

        // Option<DataKey>'s niche representation requires `lo` to be non-zero. In the
        // astronomically unlikely case of a zero first half, use 1 instead. Collision resistance
        // remains effectively 128 bits.
        let lo = NonZeroU64::new(lo).unwrap_or(NonZeroU64::new(1).unwrap());
        Self(lo, hi)
    }

    pub fn testing_new(v: u128) -> Self {
        assert_ne!(v as u64, 0);
        DataKey(NonZeroU64::new(v as u64).unwrap(), (v >> 64) as u64)
    }

    /// Reconstructs a `DataKey` from the 16-byte form written by [`bytemuck::bytes_of`] /
    /// [`bytemuck::cast_slice`].
    ///
    /// Fails if the bytes encode a zero first half. A `DataKey` always has a non-zero first half
    /// (see [`DataKey::compute`]); bytes read back from storage are untrusted, so this is checked
    /// here rather than assumed.
    pub fn from_stored_bytes(bytes: [u8; 16]) -> anyhow::Result<Self> {
        let lo = u64::from_ne_bytes(bytes[..8].try_into().unwrap());
        let hi = u64::from_ne_bytes(bytes[8..].try_into().unwrap());
        let Some(lo) = NonZeroU64::new(lo) else {
            return Err(anyhow::anyhow!(
                "corrupt DataKey: first half must be non-zero"
            ));
        };
        Ok(Self(lo, hi))
    }

    pub fn get(self) -> u128 {
        self.to_non_zero().get()
    }

    /// Returns the key as a `NonZeroU128`.
    pub fn to_non_zero(self) -> NonZeroU128 {
        NonZeroU128::new(((self.1 as u128) << 64) + (self.0.get() as u128))
            .expect("DataKey should never be zero")
    }
}

/// Serialized data retrieved from storage.
///
/// Contains the raw bytes and any nested arc references that need to be resolved
/// during deserialization.
#[derive(Allocative)]
pub struct PagableData {
    pub data: Vec<u8>,
    pub arcs: Vec<DataKey>,
}

impl PagableData {
    /// Computes the content-addressable key for this pagable data.
    ///
    /// The key is computed from the serialized data and nested arc keys.
    pub fn compute_key(&self) -> DataKey {
        DataKey::compute(
            self.arcs.len(),
            &self.data,
            bytemuck::cast_slice(&self.arcs),
        )
    }
}
