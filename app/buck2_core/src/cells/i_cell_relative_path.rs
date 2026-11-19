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
use std::ops::Deref;

use allocative::Allocative;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::intern_forward_rel_path::IForwardRelativePath;
use dupe::Dupe;
use pagable::Pagable;
use ref_cast::RefCast;
use serde::Deserialize;
use serde::Serialize;
use strong_hash::StrongHash;

use crate::cells::paths::CellRelativePath;

/// An interned `CellRelativePath`. This is `Copy` and wraps an
/// `IForwardRelativePath` with the cell-relative semantic.
///
/// See `IForwardRelativePath` for details on the interning strategy.
#[derive(
    Copy,
    Clone,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    StrongHash,
    PartialOrd,
    Ord,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ICellRelativePath {
    #[pagable(flatten_serde)]
    path: IForwardRelativePath,
}

impl ICellRelativePath {
    #[inline]
    pub const fn empty() -> Self {
        ICellRelativePath {
            path: IForwardRelativePath::empty(),
        }
    }

    #[inline]
    pub fn intern_directly(path: &CellRelativePath) -> Self {
        ICellRelativePath {
            path: IForwardRelativePath::intern_directly(path.as_forward_relative_path()),
        }
    }

    pub fn intern_concat_directly<'a, I>(parts: I) -> Self
    where
        I: IntoIterator<Item = &'a ForwardRelativePath>,
        I::IntoIter: Clone,
    {
        ICellRelativePath {
            path: IForwardRelativePath::intern_concat_directly(parts),
        }
    }

    /// See `IForwardRelativePath::intern_concat_with_tails`.
    pub fn intern_concat_with_tails<'a, I, const N: usize>(
        parts: I,
        tail_skips: [usize; N],
    ) -> Option<(ICellRelativePath, [ICellRelativePath; N])>
    where
        I: IntoIterator<Item = &'a ForwardRelativePath>,
        I::IntoIter: Clone,
    {
        let (parent, tails) = IForwardRelativePath::intern_concat_with_tails(parts, tail_skips)?;
        Some((
            ICellRelativePath::from(parent),
            tails.map(ICellRelativePath::from),
        ))
    }

    #[inline]
    pub fn as_str(self) -> &'static str {
        self.path.as_str()
    }

    #[inline]
    pub fn as_cell_relative_path(self) -> &'static CellRelativePath {
        RefCast::ref_cast(self.path.as_forward_relative_path())
    }

    #[inline]
    pub fn as_forward_relative_path(self) -> &'static ForwardRelativePath {
        self.path.as_forward_relative_path()
    }

    #[inline]
    pub fn inner(self) -> IForwardRelativePath {
        self.path
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.path.is_empty()
    }

    /// Returns the parent path, allocation-free (no re-interning).
    #[inline]
    pub fn parent(self) -> Option<ICellRelativePath> {
        self.path.parent().map(ICellRelativePath::from)
    }
}

impl Deref for ICellRelativePath {
    type Target = CellRelativePath;

    #[inline]
    fn deref(&self) -> &CellRelativePath {
        self.as_cell_relative_path()
    }
}

impl AsRef<CellRelativePath> for ICellRelativePath {
    #[inline]
    fn as_ref(&self) -> &CellRelativePath {
        self.as_cell_relative_path()
    }
}

impl AsRef<ForwardRelativePath> for ICellRelativePath {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        self.as_forward_relative_path()
    }
}

impl fmt::Display for ICellRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_cell_relative_path(), f)
    }
}

impl fmt::Debug for ICellRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ICellRelativePath({:?})", self.as_str())
    }
}

impl From<IForwardRelativePath> for ICellRelativePath {
    #[inline]
    fn from(path: IForwardRelativePath) -> Self {
        ICellRelativePath { path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let p = ICellRelativePath::empty();
        assert!(p.is_empty());
        assert_eq!("", p.as_str());
    }

    #[test]
    fn test_intern_and_equality() {
        let a = ICellRelativePath::intern_directly(CellRelativePath::testing_new("foo/bar"));
        let b = ICellRelativePath::intern_directly(CellRelativePath::testing_new("foo/bar"));
        assert_eq!(a, b);
        assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr());
    }

    #[test]
    fn test_deref_to_cell_relative_path() {
        let p = ICellRelativePath::intern_directly(CellRelativePath::testing_new("foo/bar"));
        let crp: &CellRelativePath = &p;
        assert_eq!("foo/bar", crp.as_str());
    }

    #[test]
    fn test_display_debug() {
        let p = ICellRelativePath::intern_directly(CellRelativePath::testing_new("foo/bar"));
        assert_eq!("foo/bar", format!("{p}"));
        assert_eq!("ICellRelativePath(\"foo/bar\")", format!("{p:?}"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let p = ICellRelativePath::intern_directly(CellRelativePath::testing_new("foo/bar"));
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!("\"foo/bar\"", json);
        let deserialized: ICellRelativePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, deserialized);
    }

    #[test]
    fn test_inner_forward_path() {
        let p = ICellRelativePath::intern_directly(CellRelativePath::testing_new("foo/bar"));
        let inner = p.inner();
        assert_eq!("foo/bar", inner.as_str());
    }
}
