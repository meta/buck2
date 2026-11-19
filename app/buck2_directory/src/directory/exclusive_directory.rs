/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::directory_digest::DirectoryDigest;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use derivative::Derivative;
use derive_more::Display;
use pagable::Pagable;

use crate::directory::builder::DirectoryBuilder;
use crate::directory::dashmap_directory_interner::DashMapDirectoryInterner;
use crate::directory::directory::Directory;
use crate::directory::directory_data::DirectoryData;
use crate::directory::entry::DirectoryEntry;
use crate::directory::exhaustiveness::ExhaustivenessHash;
use crate::directory::immutable_directory::ImmutableDirectory;
use crate::directory::immutable_or_exclusive::ImmutableOrExclusiveDirectoryRef;
use crate::directory::macros::impl_fingerprinted_directory;
use crate::directory::shared_directory::SharedDirectory;
use crate::directory::shared_directory::SharedDirectoryInternable;

#[derive(Derivative, Display, Allocative, Pagable)]
#[derivative(Debug(bound = "L: ::std::fmt::Debug"))]
#[derivative(Clone(bound = "L: ::std::clone::Clone"))]
#[display("{}", self.data)]
#[pagable(bound = "L: Pagable + SharedDirectoryInternable<H>, H: Pagable")]
pub struct ExclusiveDirectory<L, H>
where
    H: DirectoryDigest,
{
    pub(super) data: DirectoryData<ImmutableDirectory<L, H>, L, H>,
}

impl<L, H> ExclusiveDirectory<L, H>
where
    H: DirectoryDigest,
{
    pub fn shared(self, interner: &DashMapDirectoryInterner<L, H>) -> SharedDirectory<L, H> {
        if let Some(shared) = interner.get(self.fingerprint(), self.exhaustiveness_hash()) {
            return shared;
        }

        let DirectoryData {
            entries,
            fingerprint,
            size,
            exhaustiveness_hash,
            _hash,
        } = self.data;

        let entries = entries
            .into_iter()
            .map(|(k, v)| (k, v.map_dir(|d| d.shared(interner))))
            .collect();

        let new_data = DirectoryData {
            entries,
            size,
            fingerprint,
            exhaustiveness_hash,
            _hash,
        };

        interner.intern(new_data)
    }

    pub fn collect_entries<C>(self) -> C
    where
        C: FromIterator<(FileNameBuf, DirectoryEntry<DirectoryBuilder<L, H>, L>)>,
    {
        self.into_entries().collect()
    }

    pub fn into_entries(
        self,
    ) -> impl Iterator<Item = (FileNameBuf, DirectoryEntry<DirectoryBuilder<L, H>, L>)> {
        self.data
            .entries
            .into_iter()
            .map(|(k, v)| (k, v.map_dir(|v| v.into_builder())))
    }

    pub fn entries(
        &self,
    ) -> impl IntoIterator<Item = (&FileNameBuf, &DirectoryEntry<ImmutableDirectory<L, H>, L>)> + '_
    {
        &self.data.entries
    }

    pub fn get<'a>(
        &'a self,
        needle: &'_ FileName,
    ) -> Option<DirectoryEntry<&'a ImmutableDirectory<L, H>, &'a L>> {
        self.data.entries.get(needle).as_ref().map(|v| v.as_ref())
    }

    pub fn fingerprint(&self) -> &H {
        self.data.fingerprint()
    }

    pub fn exhaustiveness_hash(&self) -> ExhaustivenessHash {
        self.data.exhaustiveness_hash
    }

    pub fn size(&self) -> u64 {
        self.data.size
    }

    pub fn into_builder(self) -> DirectoryBuilder<L, H> {
        DirectoryBuilder::Immutable(ImmutableDirectory::Exclusive(self))
    }
}

impl<L, H> Directory<L, H> for ExclusiveDirectory<L, H>
where
    H: DirectoryDigest,
{
    type DirectoryRef<'a>
        = ImmutableOrExclusiveDirectoryRef<'a, L, H>
    where
        Self: Sized + 'a,
        L: 'a;

    fn as_ref<'a>(&'a self) -> ImmutableOrExclusiveDirectoryRef<'a, L, H>
    where
        Self: Sized + 'a,
    {
        ImmutableOrExclusiveDirectoryRef::Exclusive(self)
    }

    fn to_builder(&self) -> DirectoryBuilder<L, H>
    where
        L: Clone,
    {
        self.clone().into_builder()
    }
}

impl_fingerprinted_directory!(ExclusiveDirectory);
