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
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;

use crate::directory::directory::Directory;

/// An entry in a Directory, parameterized by the type of children directories and the type of leaf
/// nodes. We expect to be able to traverse directories, and we don't traverse leaves.
#[derive(
    Clone,
    Dupe,
    Debug,
    Eq,
    PartialEq,
    Display,
    Hash,
    Allocative,
    Pagable,
    strong_hash::StrongHash
)]
#[display(bound(D: ::std::fmt::Display, L: ::std::fmt::Display))]
pub enum DirectoryEntry<D, L> {
    Dir(D),
    Leaf(L),
}

impl<D, L> DirectoryEntry<D, L> {
    pub fn map_dir<U>(self, f: impl FnOnce(D) -> U) -> DirectoryEntry<U, L> {
        match self {
            Self::Dir(d) => DirectoryEntry::Dir(f(d)),
            Self::Leaf(l) => DirectoryEntry::Leaf(l),
        }
    }

    pub fn map_leaf<U>(self, f: impl FnOnce(L) -> U) -> DirectoryEntry<D, U> {
        match self {
            Self::Dir(d) => DirectoryEntry::Dir(d),
            Self::Leaf(l) => DirectoryEntry::Leaf(f(l)),
        }
    }

    pub fn as_ref(&self) -> DirectoryEntry<&'_ D, &'_ L> {
        match self {
            Self::Dir(d) => DirectoryEntry::Dir(d),
            Self::Leaf(l) => DirectoryEntry::Leaf(l),
        }
    }

    pub fn as_ref_dyn<H>(&self) -> DirectoryEntry<&dyn Directory<L, H>, &L>
    where
        D: Directory<L, H>,
    {
        match self {
            Self::Dir(d) => DirectoryEntry::Dir(d),
            Self::Leaf(l) => DirectoryEntry::Leaf(l),
        }
    }

    pub fn as_mut(&mut self) -> DirectoryEntry<&'_ mut D, &'_ mut L> {
        match self {
            Self::Dir(d) => DirectoryEntry::Dir(d),
            Self::Leaf(l) => DirectoryEntry::Leaf(l),
        }
    }

    pub fn dir(self) -> Option<D> {
        match self {
            Self::Dir(d) => Some(d),
            Self::Leaf(..) => None,
        }
    }

    pub fn leaf(self) -> Option<L> {
        match self {
            Self::Dir(..) => None,
            Self::Leaf(l) => Some(l),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::directory::entry::DirectoryEntry;
    use crate::directory::exclusive_directory::ExclusiveDirectory;
    use crate::directory::immutable_directory::ImmutableDirectory;
    use crate::directory::shared_directory::SharedDirectory;
    use crate::directory::test::NopEntry;
    use crate::directory::test::TestDigest;

    #[test]
    fn test_bounds() {
        fn assert_impls_eq<T: std::cmp::Eq>() {}

        assert_impls_eq::<DirectoryEntry<ExclusiveDirectory<NopEntry, TestDigest>, NopEntry>>();
        assert_impls_eq::<DirectoryEntry<SharedDirectory<NopEntry, TestDigest>, NopEntry>>();
        assert_impls_eq::<DirectoryEntry<ImmutableDirectory<NopEntry, TestDigest>, NopEntry>>();
    }
}
