/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::directory_digest::DirectoryDigest;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use derivative::Derivative;
use dupe::Clone_;
use dupe::Copy_;

use crate::directory::directory::Directory;
use crate::directory::directory_ref::DirectoryRef;
use crate::directory::directory_ref::FingerprintedDirectoryRef;
use crate::directory::entry::DirectoryEntry;
use crate::directory::exclusive_directory::ExclusiveDirectory;
use crate::directory::fingerprinted_directory::FingerprintedDirectory;
use crate::directory::immutable_directory::ImmutableDirectory;
use crate::directory::shared_directory::SharedDirectory;

pub enum ImmutableOrExclusiveDirectoryEntries<'a, L, H>
where
    H: DirectoryDigest,
{
    Immutable(
        crate::directory::sorted_slice_map::Iter<
            'a,
            FileNameBuf,
            DirectoryEntry<ImmutableDirectory<L, H>, L>,
        >,
    ),
    Shared(
        crate::directory::sorted_slice_map::Iter<
            'a,
            FileNameBuf,
            DirectoryEntry<SharedDirectory<L, H>, L>,
        >,
    ),
}

impl<'a, L, H> Iterator for ImmutableOrExclusiveDirectoryEntries<'a, L, H>
where
    H: DirectoryDigest,
{
    type Item = (
        &'a FileName,
        DirectoryEntry<ImmutableOrExclusiveDirectoryRef<'a, L, H>, &'a L>,
    );

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Immutable(iter) => {
                let (name, entry) = iter.next()?;
                Some((
                    name,
                    entry
                        .as_ref()
                        .map_dir(ImmutableOrExclusiveDirectoryRef::from_immutable),
                ))
            }
            Self::Shared(iter) => {
                let (name, entry) = iter.next()?;
                Some((
                    name,
                    entry
                        .as_ref()
                        .map_dir(ImmutableOrExclusiveDirectoryRef::Shared),
                ))
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Immutable(iter) => iter.size_hint(),
            Self::Shared(iter) => iter.size_hint(),
        }
    }
}

#[derive(Copy_, Clone_, Derivative)]
#[derivative(Debug(bound = "L: ::std::fmt::Debug"))]
pub enum ImmutableOrExclusiveDirectoryRef<'a, L, H>
where
    H: DirectoryDigest,
{
    Exclusive(&'a ExclusiveDirectory<L, H>),
    Shared(&'a SharedDirectory<L, H>),
}

impl<'a, L, H> ImmutableOrExclusiveDirectoryRef<'a, L, H>
where
    H: DirectoryDigest,
{
    pub fn from_immutable(dir: &'a ImmutableDirectory<L, H>) -> Self {
        match dir {
            ImmutableDirectory::Exclusive(d) => Self::Exclusive(d),
            ImmutableDirectory::Shared(d) => Self::Shared(d),
        }
    }
}

impl<'a, L, H> DirectoryRef<'a> for ImmutableOrExclusiveDirectoryRef<'a, L, H>
where
    H: DirectoryDigest,
{
    type Leaf = L;
    type DirectoryDigest = H;
    type Entries = ImmutableOrExclusiveDirectoryEntries<'a, L, H>;

    fn get(self, name: &FileName) -> Option<DirectoryEntry<Self, &'a Self::Leaf>> {
        match self {
            Self::Exclusive(dir) => dir
                .get(name)
                .map(|entry| entry.map_dir(ImmutableOrExclusiveDirectoryRef::from_immutable)),
            Self::Shared(dir) => dir
                .get(name)
                .map(|entry| entry.map_dir(ImmutableOrExclusiveDirectoryRef::Shared)),
        }
    }

    fn entries(self) -> Self::Entries {
        match self {
            Self::Exclusive(dir) => {
                ImmutableOrExclusiveDirectoryEntries::Immutable(dir.data.entries.iter())
            }
            Self::Shared(dir) => {
                ImmutableOrExclusiveDirectoryEntries::Shared(dir.inner.data.entries.iter())
            }
        }
    }

    fn as_dyn(self) -> &'a dyn Directory<Self::Leaf, Self::DirectoryDigest> {
        match self {
            Self::Exclusive(dir) => dir,
            Self::Shared(dir) => dir,
        }
    }
}

impl<'a, L, H> FingerprintedDirectoryRef<'a> for ImmutableOrExclusiveDirectoryRef<'a, L, H>
where
    H: DirectoryDigest,
{
    fn as_fingerprinted_dyn(
        self,
    ) -> &'a dyn FingerprintedDirectory<Self::Leaf, Self::DirectoryDigest> {
        match self {
            Self::Exclusive(dir) => dir,
            Self::Shared(dir) => dir,
        }
    }
}
