/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;

use buck2_core::directory_digest::DirectoryDigest;

use crate::directory::directory::Directory;
use crate::directory::directory_ref::FingerprintedDirectoryRef;
use crate::directory::exhaustiveness::ExhaustivenessHash;

pub trait FingerprintedDirectory<L, H>: Directory<L, H> {
    type FingerprintedDirectoryRef<'a>: FingerprintedDirectoryRef<'a, Leaf = L, DirectoryDigest = H>
    where
        Self: Sized + 'a,
        L: 'a;

    fn as_fingerprinted_ref<'a>(&'a self) -> Self::FingerprintedDirectoryRef<'a>
    where
        Self: Sized + 'a;

    fn fingerprint(&self) -> &H
    where
        H: DirectoryDigest;

    /// The second half of this directory's identity; see [`ExhaustivenessHash`]. Two
    /// fingerprinted directories are the same directory iff both their fingerprints and their
    /// exhaustiveness hashes agree.
    fn exhaustiveness_hash(&self) -> ExhaustivenessHash;

    fn size(&self) -> u64;
}

impl<L, H> fmt::Debug for &dyn FingerprintedDirectory<L, H> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "FingerprintedDirectory")
    }
}
