/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_fs::paths::file_name::FileName;

use crate::directory::directory_ref::DirectoryRef;
use crate::directory::entry::DirectoryEntry;
use crate::directory::path_accumulator::PathAccumulator;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum DirectoryFindError {
    #[error("Find would traverse a leaf at path: `{}`", .path)]
    CannotTraverseLeaf { path: PathAccumulator },
}

trait FindConflict<T> {
    /// `leaf_name` is the name of the leaf that blocks the lookup; `next` and `remaining` are the
    /// components of the requested path that follow it.
    fn new<'b>(
        leaf_name: &'b FileName,
        next: &'b FileName,
        remaining: impl Iterator<Item = &'b FileName>,
        leaf: T,
    ) -> Self;

    fn with(self, path: &FileName) -> Self;
}

impl<T> FindConflict<T> for PathAccumulator {
    fn new<'b>(
        leaf_name: &'b FileName,
        _next: &'b FileName,
        _remaining: impl Iterator<Item = &'b FileName>,
        _leaf: T,
    ) -> Self {
        PathAccumulator::new(leaf_name)
    }

    fn with(self, path: &FileName) -> Self {
        PathAccumulator::with(self, path)
    }
}

#[cfg(test)]
struct PrefixLookupContainer<T> {
    leaf: T,
    path: buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf,
}

#[cfg(test)]
impl<T> FindConflict<T> for PrefixLookupContainer<T> {
    fn new<'b>(
        _leaf_name: &'b FileName,
        next: &'b FileName,
        remaining: impl Iterator<Item = &'b FileName>,
        leaf: T,
    ) -> Self {
        Self {
            leaf,
            path: std::iter::once(next).chain(remaining).collect(),
        }
    }

    fn with(self, _path: &FileName) -> Self {
        self
    }
}

pub fn find<'a, 'b, D: DirectoryRef<'a>>(
    dir: D,
    path: impl IntoIterator<Item = &'b FileName>,
) -> Result<Option<DirectoryEntry<D, &'a D::Leaf>>, DirectoryFindError> {
    let mut path = path.into_iter();

    let path_needle = match path.next() {
        Some(path_needle) => path_needle,
        None => return Ok(Some(DirectoryEntry::Dir(dir))),
    };

    find_inner::<_, PathAccumulator>(dir, path_needle, path)
        .map_err(move |path| DirectoryFindError::CannotTraverseLeaf { path })
}

#[cfg(test)] // Dead code.
pub(crate) fn find_prefix<'a, 'b, D: DirectoryRef<'a>>(
    dir: D,
    path: impl IntoIterator<Item = &'b FileName>,
) -> Result<
    Option<(
        DirectoryEntry<D, &'a D::Leaf>,
        // Remaining path.
        buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf,
    )>,
    DirectoryFindError,
> {
    let mut path = path.into_iter();

    let path_needle = match path.next() {
        Some(path_needle) => path_needle,
        None => {
            return Ok(Some((
                DirectoryEntry::Dir(dir),
                buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf::default(),
            )));
        }
    };

    match find_inner::<_, PrefixLookupContainer<&'a D::Leaf>>(dir, path_needle, path) {
        Ok(maybe_leaf) => Ok(maybe_leaf.map(|l| {
            (
                l,
                buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf::default(),
            )
        })),
        Err(PrefixLookupContainer { leaf, path }) => Ok(Some((DirectoryEntry::Leaf(leaf), path))),
    }
}

fn find_inner<'a, 'b, D: DirectoryRef<'a>, A>(
    dir: D,
    path_needle: &'b FileName,
    mut path_rest: impl Iterator<Item = &'b FileName>,
) -> Result<Option<DirectoryEntry<D, &'a D::Leaf>>, A>
where
    A: FindConflict<&'a D::Leaf>,
{
    let entry = match dir.get(path_needle) {
        Some(entry) => entry,
        None => return Ok(None),
    };

    let next_path_needle = match path_rest.next() {
        Some(next_path_needle) => next_path_needle,
        None => return Ok(Some(entry)),
    };

    match entry {
        DirectoryEntry::Dir(dir) => find_inner::<_, A>(dir, next_path_needle, path_rest)
            .map_err(|acc| acc.with(path_needle)),
        DirectoryEntry::Leaf(leaf) => Err(A::new(path_needle, next_path_needle, path_rest, leaf)),
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use crate::directory::directory::Directory;
    use crate::directory::entry::DirectoryEntry;
    use crate::directory::find::DirectoryFindError;
    use crate::directory::find::find;
    use crate::directory::find::find_prefix;
    use crate::directory::test::NopEntry;
    use crate::directory::test::TestDirectoryBuilder;
    use crate::directory::test::path;

    #[test]
    fn test_find_error_names_blocking_leaf() -> buck2_error::Result<()> {
        // The error names the leaf that blocks the lookup.
        let mut a = TestDirectoryBuilder::empty_non_exhaustive();
        a.insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))?;
        assert_matches!(
            find(a.as_ref(), path("a/b/c")),
            Err(DirectoryFindError::CannotTraverseLeaf { path }) => {
                assert_eq!("a/b", path.to_string());
            }
        );
        assert_matches!(
            a.remove_prefix(path("a/b/c")),
            Err(DirectoryFindError::CannotTraverseLeaf { path }) => {
                assert_eq!("a/b", path.to_string());
            }
        );

        let mut a = TestDirectoryBuilder::empty_non_exhaustive();
        a.insert(path("a"), DirectoryEntry::Leaf(NopEntry))?;
        assert_matches!(
            find(a.as_ref(), path("a/b")),
            Err(DirectoryFindError::CannotTraverseLeaf { path }) => {
                assert_eq!("a", path.to_string());
            }
        );

        Ok(())
    }

    #[test]
    fn test_find() -> buck2_error::Result<()> {
        let mut a = TestDirectoryBuilder::empty_non_exhaustive();
        a.insert(path("a/b/c"), DirectoryEntry::Leaf(NopEntry))?;

        assert_matches!(
            find(a.as_ref(), path("a/b/c")),
            Ok(Some(DirectoryEntry::Leaf(..)))
        );

        assert_matches!(
            find(a.as_ref(), path("a/b")),
            Ok(Some(DirectoryEntry::Dir(..)))
        );

        assert_matches!(
            find(a.as_ref(), path("")),
            Ok(Some(DirectoryEntry::Dir(..)))
        );

        Ok(())
    }

    #[test]
    fn test_find_prefix() -> buck2_error::Result<()> {
        let mut a = TestDirectoryBuilder::empty_non_exhaustive();
        a.insert(path("a/b/c"), DirectoryEntry::Leaf(NopEntry))?;

        assert_matches!(
            find_prefix(a.as_ref(), path("a/b/c")),
            Ok(Some((
                DirectoryEntry::Leaf(..),
                path
            ))) if path.is_empty()
        );
        assert_matches!(
            find_prefix(a.as_ref(), path("a/b")),
            Ok(Some((
                DirectoryEntry::Dir(..),
                path
            ))) if path.is_empty()
        );

        assert_matches!(
            find_prefix(a.as_ref(), path("a/b/c/d")),
            Ok(Some((DirectoryEntry::Leaf(..), rest))) => {
                assert_eq!(rest, path("d"));
            }
        );
        assert_matches!(
            find_prefix(a.as_ref(), path("a/b/c/d/e")),
            Ok(Some((DirectoryEntry::Leaf(..), rest))) => {
                assert_eq!(rest, path("d/e"));
            }
        );

        Ok(())
    }
}
