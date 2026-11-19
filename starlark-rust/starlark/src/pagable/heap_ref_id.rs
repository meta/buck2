/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Deduplication and non-inline serialization support for `OwnedFrozen<()>`.
//!
//! Heaps are serialized in a separate heap table (topologically sorted,
//! dependencies first), and `OwnedFrozen<()>` references within arena values
//! are just `HeapRefId`s pointing into the table.
//!
//! ## Wire format
//!
//! ```text
//! Heap Table:
//!   [heap_count: u32]
//!   for each heap (topo order, dependencies first):
//!     [HeapRefId: u64]
//!     [FrozenFrozenHeap arena data...]
//!
//! `OwnedFrozen<()>` reference (within arena values):
//!   [u8: tag]
//!     0 = None (empty heap ref)
//!     1 = Ref  [HeapRefId: u64]
//! ```

use std::hash::Hasher;

use allocative::Allocative;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use strong_hash::StrongHash;

use crate::values::FrozenHeapName;
use crate::values::layout::heap::sealed::HeapSerializationNonce;

/// The identity a serialized pointer names its heap by.
///
/// A name alone does not identify a heap: the same module re-evaluated after
/// an invalidation seals another heap under the same name with different
/// content, and a pointer written against one must never resolve into the
/// other. So the id covers the name and the nonce drawn when the heap was
/// sealed.
/// Hashed with blake3 via [`StrongHash`] so an id is a function of what it
/// names, not of the process that computed it.
#[derive(
    Debug,
    Clone,
    Copy,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    Allocative,
    PagableSerialize,
    PagableDeserialize
)]
pub struct HeapRefId(u64);

impl HeapRefId {
    /// The hash of a name by itself. Not a heap's identity - that is
    /// [`new`](Self::new), through `FrozenHeapArc::heap_ref_id` - but the way
    /// to ask whether two heaps share a name.
    #[cfg(test)]
    pub(crate) fn from_heap_name(name: &FrozenHeapName) -> Self {
        let mut hasher = Blake3StrongHasher::new();
        name.strong_hash(&mut hasher);
        Self(hasher.finish())
    }

    /// The identity of the heap sealed under `name` with `nonce`.
    pub(crate) fn new(name: &FrozenHeapName, nonce: HeapSerializationNonce) -> Self {
        let mut hasher = Blake3StrongHasher::new();
        name.strong_hash(&mut hasher);
        hasher.write(&nonce.to_le_bytes());
        Self(hasher.finish())
    }
}

/// `std::hash::Hasher` adapter over `blake3::Hasher`. Used to drive
/// [`StrongHash`] implementations into a deterministic blake3 digest.
#[derive(Default)]
pub(crate) struct Blake3StrongHasher(blake3::Hasher);

impl Blake3StrongHasher {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

// Only `write` and `finish` are forwarded; the `write_*` helpers fall back to
// the default impls. blake3 ingests bytes uniformly, so the defaults are fine.
impl Hasher for Blake3StrongHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(&self) -> u64 {
        let bytes = self.0.finalize().as_bytes()[..8]
            .try_into()
            .expect("blake3 digest is at least 8 bytes");
        u64::from_be_bytes(bytes)
    }
}
