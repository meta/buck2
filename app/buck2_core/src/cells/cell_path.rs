/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::OnceLock;

use allocative::Allocative;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::cells::CellResolver;
use crate::cells::i_cell_relative_path::ICellRelativePath;
use crate::cells::name::CellName;
use crate::cells::paths::CellRelativePath;

#[derive(buck2_error::Error, Debug)]
#[error("attempted to strip prefix of two CellPath with different cell names `{0}` and `{1}`")]
#[buck2(tag = Tier0)]
struct StripPrefixError(CellName, CellName);

pub(crate) fn set_optimization_hint_cell_resolver(resolver: &CellResolver) {
    OPTIMIZATION_HINT_CELL_RESOLVER.get_or_init(|| resolver.dupe());
}

/// Below, we would like to have access to a `CellResolver` when constructing `CellPath`s. However,
/// in practice that's unrealistic with the way things are set up right now. So instead, we take
/// advantage of the fact that cell resolvers ~never change and so we do the slightly jank thing of
/// just stashing one here and using it forever. In the case where it does eventually change some of
/// our optimizations are invalidated but it's otherwise fine.
static OPTIMIZATION_HINT_CELL_RESOLVER: OnceLock<CellResolver> = OnceLock::new();

/// Represents a resolvable path corresponding to some path that is relative to the cell
/// corresponding to the 'CellName'.
#[derive(
    Copy,
    Clone,
    Dupe,
    Debug,
    derive_more::Display,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative,
    StrongHash,
    Pagable
)]
#[display("{cell}//{path}")]
pub struct CellPath {
    cell: CellName,
    path: ICellRelativePath,
}

impl CellPath {
    #[inline]
    pub fn new_i(cell: CellName, path: ICellRelativePath) -> Self {
        Self { cell, path }
    }

    #[inline]
    pub fn new(cell: CellName, path: &CellRelativePath) -> Self {
        Self::new_parts(cell, &[path.as_forward_relative_path()])
    }

    fn new_parts(cell: CellName, parts: &[&ForwardRelativePath]) -> Self {
        if let Some(resolver) = OPTIMIZATION_HINT_CELL_RESOLVER.get() {
            if let Some(root) = resolver.get(cell).ok().map(|c| c.path()) {
                // Optimization: Whenever we construct a cell path `fbcode//a/b` we generally expect
                // that we will eventually need to construct a `ProjectRelativePath` `fbcode/a/b` to
                // go with it. Here, we make an effort to pre-fill the full project relative path
                // into the interner and have our path be a view into it. That way, when it becomes
                // time to construct the project relative path later, we'll get a hit in the
                // interner and won't need to pay duplicate memory.
                let p = ICellRelativePath::intern_concat_with_tails(
                    std::iter::once(root.as_forward_relative_path()).chain(parts.iter().copied()),
                    [1],
                )
                .expect("had one component")
                .1[0];
                return CellPath { cell, path: p };
            }
        }
        let p = ICellRelativePath::intern_concat_directly(parts.iter().copied());
        CellPath { cell, path: p }
    }

    /// A `CellPath` at the root of the given cell (empty relative path).
    #[inline]
    pub fn empty(cell: CellName) -> Self {
        CellPath {
            cell,
            path: ICellRelativePath::empty(),
        }
    }

    pub fn testing_new(path: &str) -> CellPath {
        let (cell_name, relative_path) = path.split_once("//").unwrap();
        CellPath::new(
            CellName::testing_new(cell_name),
            CellRelativePath::testing_new(relative_path),
        )
    }

    #[inline]
    pub fn cell(self) -> CellName {
        self.cell
    }

    #[inline]
    pub fn path(self) -> &'static CellRelativePath {
        self.path.as_cell_relative_path()
    }

    #[inline]
    pub fn ipath(self) -> ICellRelativePath {
        self.path
    }

    /// Creates a 'CellPath' with `path` adjoined to self.
    ///
    /// ```
    /// use buck2_core::cells::cell_path::CellPath;
    /// use buck2_core::cells::name::CellName;
    /// use buck2_core::cells::paths::CellRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
    ///
    /// let path = CellPath::new(
    ///     CellName::testing_new("cell"),
    ///     CellRelativePath::unchecked_new("foo/bar"),
    /// );
    /// let other = ForwardRelativePath::new("baz")?;
    /// assert_eq!(
    ///     CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("foo/bar/baz")
    ///     ),
    ///     path.join(other)
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn join<P: AsRef<ForwardRelativePath>>(self, path: P) -> CellPath {
        CellPath::new_parts(self.cell, &[self.path.as_ref(), path.as_ref()])
    }

    /// Returns the parent directory.
    ///
    /// Allocation-free (does not re-intern).
    ///
    /// ```
    /// use buck2_core::cells::cell_path::CellPath;
    /// use buck2_core::cells::name::CellName;
    /// use buck2_core::cells::paths::CellRelativePath;
    ///
    /// assert_eq!(
    ///     Some(CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("foo")
    ///     )),
    ///     CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("foo/bar")
    ///     )
    ///     .parent(),
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn parent(self) -> Option<CellPath> {
        Some(CellPath {
            cell: self.cell,
            path: self.path.parent()?,
        })
    }

    /// Produces an iterator over CellPath and its ancestors.
    ///
    /// The iterator will yield all the CellPaths that is returned if
    /// the parent method is used zero or more times in order.
    ///
    /// ```
    /// use buck2_core::cells::cell_path::CellPath;
    ///
    /// let path = CellPath::testing_new("cell//foo/bar");
    /// let mut ancestors = path.ancestors();
    ///
    /// assert_eq!(
    ///     ancestors.next(),
    ///     Some(CellPath::testing_new("cell//foo/bar"))
    /// );
    /// assert_eq!(ancestors.next(), Some(CellPath::testing_new("cell//foo")));
    /// assert_eq!(ancestors.next(), Some(CellPath::testing_new("cell//")));
    /// assert_eq!(ancestors.next(), None);
    ///
    /// # buck2_error::Ok(())
    /// ```
    pub fn ancestors(self) -> impl Iterator<Item = CellPath> {
        struct Ancestors(Option<CellPath>);
        impl Iterator for Ancestors {
            type Item = CellPath;

            fn next(&mut self) -> Option<Self::Item> {
                let current = self.0?;
                self.0 = current.parent();
                Some(current)
            }
        }

        Ancestors(Some(self))
    }

    /// Returns a 'ForwardRelativePath' that, when joined onto `base`, yields
    /// `self`.
    ///
    /// Error if `base` is not a prefix of `self` or the returned
    /// path is not a 'ForwardRelativePath'
    ///
    /// ```
    /// use buck2_core::cells::cell_path::CellPath;
    /// use buck2_core::cells::name::CellName;
    /// use buck2_core::cells::paths::CellRelativePath;
    /// use buck2_fs::paths::forward_rel_path::ForwardRelativePathBuf;
    ///
    /// let path = CellPath::new(
    ///     CellName::testing_new("cell"),
    ///     CellRelativePath::unchecked_new("test/haha/foo.txt"),
    /// );
    ///
    /// assert_eq!(
    ///     path.strip_prefix(CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("test"),
    ///     ))?,
    ///     ForwardRelativePathBuf::unchecked_new("haha/foo.txt".into())
    /// );
    /// assert!(
    ///     path.strip_prefix(CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("asdf"),
    ///     ))
    ///     .is_err()
    /// );
    /// assert!(
    ///     path.strip_prefix(CellPath::new(
    ///         CellName::testing_new("another"),
    ///         CellRelativePath::unchecked_new("test"),
    ///     ))
    ///     .is_err()
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn strip_prefix(self, base: CellPath) -> buck2_error::Result<&'static ForwardRelativePath> {
        if self.cell != base.cell {
            return Err(StripPrefixError(self.cell, base.cell).into());
        }
        self.path
            .as_cell_relative_path()
            .strip_prefix(base.path.as_cell_relative_path())
    }

    /// Checks that cell matches and `self` path starts with `base` path.
    ///
    /// ```
    /// use std::convert::TryFrom;
    ///
    /// use buck2_core::cells::cell_path::CellPath;
    /// use buck2_core::cells::name::CellName;
    /// use buck2_core::cells::paths::CellRelativePath;
    ///
    /// assert!(
    ///     CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("foo/bar")
    ///     )
    ///     .starts_with(CellPath::new(
    ///         CellName::testing_new("cell"),
    ///         CellRelativePath::unchecked_new("foo")
    ///     )),
    /// );
    ///
    /// # buck2_error::Ok(())
    /// ```
    #[inline]
    pub fn starts_with(self, base: CellPath) -> bool {
        self.cell == base.cell && self.path().starts_with(base.path())
    }

    #[inline]
    pub fn ends_with(self, suffix: &ForwardRelativePath) -> bool {
        self.path().ends_with(suffix)
    }
}

#[cfg(test)]
mod tests {
    use buck2_fs::paths::intern_forward_rel_path::IForwardRelativePath;

    use super::*;
    use crate::cells::CellResolver;
    use crate::cells::cell_root_path::CellRootPathBuf;
    use crate::fs::project_rel_path::ProjectRelativePathBuf;

    /// Construct a `CellPath` and verify that its bytes are a sub-slice of
    /// the project-relative form's interned allocation — i.e., the
    /// `OPTIMIZATION_HINT_CELL_RESOLVER` path was taken and the project
    /// form has been pre-interned at the same time.
    #[test]
    fn test_new_shares_alloc_with_project_relative_form() {
        // The hint resolver is a process-wide `OnceLock` and only the first
        // installer wins. We try to install ours; if another test got there
        // first we just reuse a cell from whichever resolver is installed.
        let our_cell = CellName::testing_new("optfire_cell_4f9e");
        let our_prefix = "optfire_prefix_4f9e";
        let _resolver = CellResolver::testing_with_name_and_path(
            our_cell,
            CellRootPathBuf::new(ProjectRelativePathBuf::unchecked_new(our_prefix.to_owned())),
        );

        let resolver = OPTIMIZATION_HINT_CELL_RESOLVER
            .get()
            .expect("a CellResolver should now be installed");

        // Pick any cell from the installed resolver whose project-relative
        // prefix is non-empty (otherwise the optimization is a no-op:
        // parent and tail would alias trivially).
        let (cell, prefix) = resolver
            .cells()
            .find_map(|(name, instance)| {
                let p = instance.path().as_str();
                (!p.is_empty()).then(|| (name, p.to_owned()))
            })
            .expect("installed resolver should have a cell with a non-empty prefix");

        // Unique cell-relative segment so that no prior interned entry
        // shadows our fresh insert.
        let inner = "optfire_inner_4f9e";
        let cp = CellPath::new(cell, CellRelativePath::testing_new(inner));

        // The construction above should have allocated `{prefix}/{inner}`
        // in the interner with `cp.path()` viewing into it at the tail.
        // Re-interning the project-relative form should hit that entry;
        // the cell-relative bytes should be exactly `prefix.len() + 1`
        // bytes past its start.
        let project = IForwardRelativePath::intern_directly(
            ForwardRelativePath::new(&format!("{prefix}/{inner}")).unwrap(),
        );
        assert_eq!(
            cp.path().as_str().as_ptr(),
            unsafe { project.as_str().as_ptr().add(prefix.len() + 1) },
            "cell-relative bytes should sit inside the project-relative allocation",
        );
    }
}
