/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::marker::PhantomData;

use allocative::Allocative;
use buck2_core::directory_digest::DirectoryDigest;
use buck2_fs::paths::file_name::FileNameBuf;
use derivative::Derivative;
use derive_more::Display;
use pagable::Pagable;

use crate::directory::directory_hasher::DirectoryDigester;
use crate::directory::entry::DirectoryEntry;
use crate::directory::exhaustiveness::Exhaustiveness;
use crate::directory::exhaustiveness::ExhaustivenessHash;
use crate::directory::fingerprinted_directory::FingerprintedDirectory;
use crate::directory::sorted_slice_map::SortedSliceMap;

#[derive(Derivative, Display, Allocative, Pagable)]
#[derivative(Debug(bound = "D: ::std::fmt::Debug, L: ::std::fmt::Debug"))]
#[derivative(Clone(bound = "D: ::std::clone::Clone, L: ::std::clone::Clone"))]
#[display("Directory({})", self.fingerprint)]
pub struct DirectoryData<D, L, H>
where
    H: DirectoryDigest,
{
    pub entries: SortedSliceMap<FileNameBuf, DirectoryEntry<D, L>>,

    /// The size of the directory.
    ///
    /// This is currently the sum of the sizes of the constituent files.
    ///
    /// FIXME(JakobDegen): It'd be nice if we could account for empty-directories and non-file
    /// leaves here.
    pub(super) size: u64,

    pub(super) fingerprint: H,

    pub(super) exhaustiveness_hash: ExhaustivenessHash,

    #[derivative(Debug = "ignore")]
    pub(super) _hash: PhantomData<H>,
}

impl<D, L, H> DirectoryData<D, L, H>
where
    H: DirectoryDigest,
{
    pub fn fingerprint(&self) -> &H {
        &self.fingerprint
    }

    pub fn exhaustiveness_hash(&self) -> ExhaustivenessHash {
        self.exhaustiveness_hash
    }
}

impl<D, L, H> DirectoryData<D, L, H>
where
    H: DirectoryDigest,
    D: FingerprintedDirectory<L, H>,
{
    pub fn new(
        entries: SortedSliceMap<FileNameBuf, DirectoryEntry<D, L>>,
        hasher: &impl DirectoryDigester<L, H>,
        exhaustiveness: Exhaustiveness,
    ) -> Self {
        let fingerprint = hasher.hash_entries(
            entries
                .iter()
                .map(|(k, e)| (k.as_ref(), e.as_ref().map_dir(|d| d.as_fingerprinted_ref()))),
        );
        let size = entries
            .iter()
            .map(|(_, e)| match e {
                DirectoryEntry::Leaf(l) => hasher.leaf_size(l),
                DirectoryEntry::Dir(d) => d.size(),
            })
            .fold(0_u64, |acc, x| acc.saturating_add(x));
        let exhaustiveness_hash = ExhaustivenessHash::compute(exhaustiveness, || {
            entries.iter().filter_map(|(_, e)| match e {
                DirectoryEntry::Dir(d) => Some(d.exhaustiveness_hash()),
                DirectoryEntry::Leaf(_) => None,
            })
        });
        // Semantic invariant, monotone downward by induction. The identity hash above stays
        // sound even when this is violated (violations hash as mixed), so this is a tripwire,
        // not load-bearing.
        debug_assert!(
            matches!(exhaustiveness, Exhaustiveness::NonExhaustive)
                || entries.iter().all(|(_, e)| match e {
                    DirectoryEntry::Dir(d) => d.exhaustiveness_hash().is_exhaustive(),
                    DirectoryEntry::Leaf(_) => true,
                }),
            "Exhaustive directories must contain only exhaustive directories",
        );
        Self {
            entries,
            size,
            fingerprint,
            exhaustiveness_hash,
            _hash: PhantomData,
        }
    }
}
