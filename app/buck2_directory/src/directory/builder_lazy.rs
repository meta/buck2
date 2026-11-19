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
use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;

use crate::directory::builder::DirectoryBuilder;
use crate::directory::builder::DirectoryInsertError;
use crate::directory::builder::DirectoryMergeError;
use crate::directory::entry::DirectoryEntry;
use crate::directory::shared_directory::SharedDirectory;

pub trait DirectoryBuilderLike<D, L> {
    fn merge(&mut self, dir: D) -> Result<(), DirectoryMergeError>;

    fn insert(
        &mut self,
        path: ForwardRelativePathBuf,
        entry: DirectoryEntry<D, L>,
    ) -> Result<(), DirectoryInsertError>;
}

/// A builder similar to a `DirectoryBuilder` but with slightly different performance
/// characteristics.
///
/// The differences with a standard `DirectoryBuilder` are:
///  1. This performs significantly better for complex merges.
///  2. This does not in itself act like a directory
///  3. This requires that the pieces being merged are compatible in the sense that they agree on
///     where and what the leaves are. If this is not the case one will be picked arbitrarily.
pub struct LazyDirectoryBuilder<L, H>
where
    H: DirectoryDigest,
    L: Clone + PartialEq + Eq,
{
    // In use without fast directories
    builder: DirectoryBuilder<L, H>,
    // In use with fast directories
    to_merge: Vec<SharedDirectory<L, H>>,
    to_insert: Vec<(
        ForwardRelativePathBuf,
        DirectoryEntry<SharedDirectory<L, H>, L>,
    )>,
}

impl<L, H> LazyDirectoryBuilder<L, H>
where
    H: DirectoryDigest,
    L: Clone + PartialEq + Eq,
{
    pub fn empty() -> Self {
        Self {
            builder: DirectoryBuilder::empty_non_exhaustive(),
            to_merge: Vec::new(),
            to_insert: Vec::new(),
        }
    }

    /// Merge this directory in
    ///
    /// Supporting only `SharedDirectory`s is effectively a performance hint - we could allow any
    /// directory, but the fast paths in the directory builder can only be hit with shared
    /// directories.
    pub fn merge(&mut self, dir: SharedDirectory<L, H>) -> Result<(), DirectoryMergeError> {
        if buck2_core::faster_directories::is_enabled() {
            self.to_merge.push(dir);
            Ok(())
        } else {
            self.builder
                .merge_with_compatible_leaves(dir.into_builder())
        }
    }

    pub fn insert(
        &mut self,
        path: ForwardRelativePathBuf,
        entry: DirectoryEntry<SharedDirectory<L, H>, L>,
    ) -> Result<(), DirectoryInsertError> {
        if buck2_core::faster_directories::is_enabled() {
            self.to_insert.push((path, entry));
            Ok(())
        } else {
            self.builder
                .insert(&path, entry.map_dir(|d| d.into_builder()))
                .map(|_| ())
        }
    }

    pub fn finalize(self) -> buck2_error::Result<DirectoryBuilder<L, H>> {
        let mut to_merge = self.to_merge;
        // Interned, so the same directory is the same allocation: deduplicating by identity
        // skips reading every neighbor's fingerprint. A content-equal duplicate that is not the
        // same allocation merges twice, which is harmless.
        to_merge.sort_by(|a, b| {
            b.size()
                .cmp(&a.size())
                .then_with(|| a.allocation_ptr().cmp(&b.allocation_ptr()))
        });
        to_merge.dedup_by(|a, b| a.allocation_ptr() == b.allocation_ptr());

        let mut to_insert = self.to_insert;
        to_insert.sort_by(|(p, _), (q, _)| Ord::cmp(p, q));
        to_insert.dedup();

        // Merge items from largest to smallest, improving the ability to reuse fingerprinted
        // directories
        let mut builder = self.builder;
        for d in to_merge {
            builder.merge_with_compatible_leaves(d.into_builder())?;
        }

        for (p, e) in to_insert {
            builder.insert(&p, e.map_dir(|d| d.into_builder()))?;
        }

        Ok(builder)
    }
}

impl<L, H> DirectoryBuilderLike<SharedDirectory<L, H>, L> for LazyDirectoryBuilder<L, H>
where
    H: DirectoryDigest,
    L: Clone + PartialEq + Eq,
{
    fn insert(
        &mut self,
        path: ForwardRelativePathBuf,
        entry: DirectoryEntry<SharedDirectory<L, H>, L>,
    ) -> Result<(), DirectoryInsertError> {
        self.insert(path, entry)
    }

    fn merge(&mut self, dir: SharedDirectory<L, H>) -> Result<(), DirectoryMergeError> {
        self.merge(dir)
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use crate::directory::builder_lazy::LazyDirectoryBuilder;
    use crate::directory::dashmap_directory_interner::DashMapDirectoryInterner;
    use crate::directory::directory::Directory;
    use crate::directory::directory_iterator::DirectoryIterator;
    use crate::directory::entry::DirectoryEntry;
    use crate::directory::test::NopEntry;
    use crate::directory::test::TestDigest;
    use crate::directory::test::TestDirectoryBuilder;
    use crate::directory::test::TestHasher;
    use crate::directory::test::path;

    type LazyTestDirectoryBuilder = LazyDirectoryBuilder<NopEntry, TestDigest>;

    #[test]
    fn test_smoke() -> buck2_error::Result<()> {
        let interner = DashMapDirectoryInterner::new();

        let mut a = LazyTestDirectoryBuilder::empty();
        a.insert(path("a/b").to_buf(), DirectoryEntry::Leaf(NopEntry))?;

        let b = {
            let mut b = TestDirectoryBuilder::empty_non_exhaustive();
            b.insert(path("a/c"), DirectoryEntry::Leaf(NopEntry))?;
            b.fingerprint(&TestHasher).shared(&interner)
        };

        a.merge(b)?;

        let a = a.finalize()?;
        let mut it = a.ordered_walk().with_paths();

        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a"))
        );

        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/b"))
        );

        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/c"))
        );

        assert_matches!(it.next(), None);

        Ok(())
    }
}
