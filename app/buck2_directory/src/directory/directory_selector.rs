/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::iter;

use buck2_core::directory_digest::DirectoryDigest;
use buck2_fs::paths::IntoFileNameBufIterator;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_hash::BuckMutInsertionMap;
use either::Either;

use crate::directory::builder::DirectoryBuilder;
use crate::directory::directory::Directory;
use crate::directory::directory_iterator::DirectoryIterator;
use crate::directory::directory_iterator::DirectoryIteratorPathAccessor;
use crate::directory::directory_iterator::DirectoryIteratorPathStack;
use crate::directory::entry::DirectoryEntry;
use crate::directory::walk::OrderedDirectoryWalkType;
use crate::directory::walk::UnorderedDirectoryWalkType;
use crate::directory::walk::WalkType;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum DirectorySearchError<L> {
    #[error("Search traverses a leaf")]
    CannotTraverseLeaf { leaf: L },
}

impl<L> DirectorySearchError<L> {
    pub fn into_leaf(self) -> L {
        let Self::CannotTraverseLeaf { leaf } = self;
        leaf
    }
}

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum DirectoryFilterError {
    #[error("Filter traverses a leaf")]
    CannotTraverseLeaf,
}

/// A query builder for filtering or search operations on directories. It's a tree of paths and
/// what action we want to take on them.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DirectorySelector {
    /// Traverse only the netries that match this filename.
    Traverse(BuckMutInsertionMap<FileNameBuf, DirectorySelector>),
    /// Take this entire tree.
    Take,
}

impl DirectorySelector {
    pub fn empty() -> Self {
        Self::Traverse(Default::default())
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Traverse(d) => d.is_empty(),
            Self::Take => false,
        }
    }

    /// Add a path to this DirectorySelector.
    pub fn select(&mut self, path: impl IntoFileNameBufIterator) {
        let path = path.into_iter();
        self.select_inner(path)
    }

    fn select_inner(&mut self, path: impl IntoIterator<Item = FileNameBuf>) {
        let mut path = path.into_iter();
        let entry = match path.next() {
            Some(e) => e,
            None => {
                *self = Self::Take;
                return;
            }
        };

        match self {
            Self::Traverse(s) => s
                .entry(entry)
                .or_insert_with(DirectorySelector::empty)
                .select_inner(path),
            Self::Take => {}
        }
    }

    /// Filter a DirectoryBuilder by only retaining matching entries.
    pub fn filter<L, H>(&self, dir: &mut DirectoryBuilder<L, H>) -> Result<(), DirectoryFilterError>
    where
        L: Clone,
        H: DirectoryDigest,
    {
        let mut res = Ok(());

        match self {
            Self::Traverse(filter) => filter_inner(dir, filter, &mut res),
            Self::Take => {}
        };

        res
    }
}

fn filter_inner<L, H>(
    dir: &mut DirectoryBuilder<L, H>,
    filter: &BuckMutInsertionMap<FileNameBuf, DirectorySelector>,
    res: &mut Result<(), DirectoryFilterError>,
) where
    L: Clone,
    H: DirectoryDigest,
{
    let dir = dir.as_mut();

    let entries = std::mem::take(dir);

    for (k, mut v) in entries.into_iter() {
        let selector = match filter.get(&k) {
            Some(s) => s,
            None => continue,
        };

        match selector {
            DirectorySelector::Traverse(next_map) => match v {
                DirectoryEntry::Dir(ref mut d) => filter_inner(d, next_map, res),
                DirectoryEntry::Leaf(..) => {
                    *res = Err(DirectoryFilterError::CannotTraverseLeaf);
                }
            },
            DirectorySelector::Take => {}
        };

        dir.insert(k, v);
    }
}

/// Continue the search.
struct SearchFrame<'a, 'b, T: WalkType<'b>> {
    search: &'a BuckMutInsertionMap<FileNameBuf, DirectorySelector>,
    name: Option<&'b FileName>,
    entries: T::Entries,
}

enum SearchInner<'a, 'b, T: WalkType<'b>> {
    ReturnRoot(Option<DirectoryEntry<T::Directory, &'b T::Leaf>>),
    Stack(Vec<SearchFrame<'a, 'b, T>>),
}

pub struct Search<'a, 'b, T: WalkType<'b>> {
    inner: SearchInner<'a, 'b, T>,
}

impl<'a, 'b, T: WalkType<'b>> Search<'a, 'b, T> {
    pub fn new(selector: &'a DirectorySelector, root: T::Directory) -> Self {
        match selector {
            DirectorySelector::Traverse(search) => Search {
                inner: SearchInner::Stack(vec![SearchFrame {
                    search,
                    name: None,
                    entries: T::directory_entries(root),
                }]),
            },
            DirectorySelector::Take => Search {
                inner: SearchInner::ReturnRoot(Some(DirectoryEntry::Dir(root))),
            },
        }
    }
}

impl<'b, T: WalkType<'b>> DirectoryIterator for Search<'_, 'b, T> {
    type PathStack<'c>
        = DirectoryIteratorPathAccessor<'c, Self>
    where
        Self: 'c;
    type Item =
        Result<DirectoryEntry<T::Directory, &'b T::Leaf>, DirectorySearchError<&'b T::Leaf>>;

    fn next(&mut self) -> Option<(DirectoryIteratorPathAccessor<'_, Self>, Self::Item)> {
        match &mut self.inner {
            SearchInner::ReturnRoot(root) => {
                let root = root.take()?;
                Some((
                    DirectoryIteratorPathAccessor {
                        leaf: None,
                        stack: self,
                    },
                    Ok(root),
                ))
            }
            SearchInner::Stack(stack) => {
                loop {
                    let frame = stack.last_mut()?;

                    let SearchFrame {
                        search, entries, ..
                    } = frame;

                    if let Some((name, entry)) = entries.next() {
                        let search = search.get(name);

                        match search {
                            Some(DirectorySelector::Traverse(t)) => match entry {
                                // Traverse into this directory ... assuming it's a directory :)
                                DirectoryEntry::Dir(d) => {
                                    stack.push(SearchFrame {
                                        name: Some(name),
                                        search: t,
                                        entries: T::directory_entries(d),
                                    });
                                    continue;
                                }
                                DirectoryEntry::Leaf(leaf) => {
                                    return Some((
                                        DirectoryIteratorPathAccessor {
                                            leaf: Some(name),
                                            stack: self,
                                        },
                                        Err(DirectorySearchError::CannotTraverseLeaf { leaf }),
                                    ));
                                }
                            },
                            Some(DirectorySelector::Take) => {
                                // Return the entry. Do not traverse further.
                                return Some((
                                    DirectoryIteratorPathAccessor {
                                        leaf: Some(name),
                                        stack: self,
                                    },
                                    Ok(entry),
                                ));
                            }
                            None => {
                                // Ignore this node entirely.
                                continue;
                            }
                        }
                    }

                    // We've exhausted this iterator. Go back to the previous stack frame.
                    stack.pop();
                }
            }
        }
    }
}

impl<'b, T: WalkType<'b>> DirectoryIteratorPathStack for Search<'_, 'b, T> {
    fn path(&self) -> impl Iterator<Item = &FileName> {
        match &self.inner {
            SearchInner::ReturnRoot(_) => Either::Left(iter::empty()),
            SearchInner::Stack(stack) => Either::Right(stack.iter().filter_map(|frame| frame.name)),
        }
    }
}

pub type UnorderedDirectorySearch<'a, 'b, D> = Search<'a, 'b, UnorderedDirectoryWalkType<'b, D>>;
pub type OrderedDirectorySearch<'a, 'b, D> = Search<'a, 'b, OrderedDirectoryWalkType<'b, D>>;

impl DirectorySelector {
    pub fn unordered_search<'a, 'b, L, H, D: Directory<L, H>>(
        &'a self,
        dir: &'b D,
    ) -> UnorderedDirectorySearch<'a, 'b, D::DirectoryRef<'b>> {
        UnorderedDirectorySearch::new(self, dir.as_ref())
    }

    pub fn ordered_search<'a, 'b, L, H, D: Directory<L, H>>(
        &'a self,
        dir: &'b D,
    ) -> OrderedDirectorySearch<'a, 'b, D::DirectoryRef<'b>> {
        OrderedDirectorySearch::new(self, dir.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use crate::directory::directory::Directory;
    use crate::directory::directory_iterator::DirectoryIterator;
    use crate::directory::directory_selector::DirectorySearchError;
    use crate::directory::directory_selector::DirectorySelector;
    use crate::directory::entry::DirectoryEntry;
    use crate::directory::test::NopEntry;
    use crate::directory::test::TestDirectoryBuilder;
    use crate::directory::test::TestHasher;
    use crate::directory::test::path;

    #[test]
    fn test_search() -> buck2_error::Result<()> {
        let mut b = TestDirectoryBuilder::empty_non_exhaustive();
        b.insert(path("a/b"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("b/c"), DirectoryEntry::Leaf(NopEntry))?;
        let d = b.fingerprint(&TestHasher);

        {
            let mut selector = DirectorySelector::empty();
            selector.select(path("a/b"));

            let mut it = selector.ordered_search(&d).with_paths();

            assert_matches!(
                it.next(),
                Some((p, Ok(DirectoryEntry::Leaf(..)))) => assert_eq!(p, path("a/b"))
            );
            assert_matches!(it.next(), None)
        }

        {
            let mut selector = DirectorySelector::empty();
            selector.select(path("a/b/c"));
            selector.select(path("b/c"));

            let mut it = selector.ordered_search(&d).with_paths();

            assert_matches!(
                it.next(),
                Some((p, Err(DirectorySearchError::CannotTraverseLeaf { .. }))) => assert_eq!(p, path("a/b"))
            );
            assert_matches!(
                it.next(),
                Some((p, Ok(DirectoryEntry::Leaf(..)))) => assert_eq!(p, path("b/c"))
            );
            assert_matches!(it.next(), None)
        }

        {
            let mut selector = DirectorySelector::empty();
            selector.select(path("a"));

            let mut it = selector.ordered_search(&d).with_paths();
            assert_matches!(
                it.next(),
                Some((p, Ok(DirectoryEntry::Dir(..)))) => assert_eq!(p, path("a"))
            );
        }

        Ok(())
    }

    #[test]
    fn test_filter() -> buck2_error::Result<()> {
        let mut b = TestDirectoryBuilder::empty_non_exhaustive();
        b.insert(path("a/aa"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("a/a"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("b/b"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("b/bb"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("c/c"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("c/cc"), DirectoryEntry::Leaf(NopEntry))?;

        let mut selector = DirectorySelector::empty();
        selector.select(path("a"));
        selector.select(path("b/b"));

        selector.filter(&mut b)?;

        let mut it = b.ordered_walk().with_paths();

        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/a"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/aa"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("b"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("b/b"))
        );

        assert_matches!(it.next(), None);

        Ok(())
    }

    #[test]
    fn test_filter_continues_on_error() -> buck2_error::Result<()> {
        let mut b = TestDirectoryBuilder::empty_non_exhaustive();
        b.insert(path("a/aa/aaa"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("a/aa/bbb"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("a/bb"), DirectoryEntry::Leaf(NopEntry))?;
        b.insert(path("c"), DirectoryEntry::Leaf(NopEntry))?;

        let mut selector = DirectorySelector::empty();
        selector.select(path("a/aa"));
        selector.select(path("c/d"));

        assert_matches!(selector.filter(&mut b), Err(..));

        let mut it = b.ordered_walk().with_paths();

        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/aa"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/aa/aaa"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("a/aa/bbb"))
        );
        assert_matches!(
            it.next(),
            Some((p, _)) => assert_eq!(p, path("c"))
        );

        assert_matches!(it.next(), None);

        Ok(())
    }
}
