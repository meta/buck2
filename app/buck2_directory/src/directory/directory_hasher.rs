/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::fmt::Debug;
use std::hash::Hash;

use allocative::Allocative;
use buck2_core::directory_digest::DirectoryDigest;
use buck2_fs::paths::file_name::FileName;
use derive_more::Display;
use dupe::Dupe;

use crate::directory::directory_ref::FingerprintedDirectoryRef;
use crate::directory::entry::DirectoryEntry;

pub trait DirectoryDigester<L, H> {
    fn hash_entries<'a, D, I>(&self, entries: I) -> H
    where
        I: IntoIterator<Item = (&'a FileName, DirectoryEntry<D, &'a L>)>,
        D: FingerprintedDirectoryRef<'a, Leaf = L, DirectoryDigest = H> + 'a,
        L: 'a,
        Self: Sized;

    /// Return the "size" of a leaf.
    ///
    /// These values are aggregated up to eventually become the size of the fingerprinted directory.
    ///
    /// It may appear unusual to have this on the digester, but the digester is the only part of the
    /// directory infrastructure that actually understand what the leaves are
    fn leaf_size(&self, leaf: &L) -> u64;
}

#[allow(unused)]
#[derive(Clone, Debug, Eq, PartialEq, Hash, Allocative, Display)]
pub struct NoDigest(Infallible);

impl Dupe for NoDigest {}

impl DirectoryDigest for NoDigest {}
