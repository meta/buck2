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
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::paths::intern_forward_rel_path::IForwardRelativePath;
use dupe::Dupe;
use pagable::Pagable;
use ref_cast::RefCast;
use serde::Deserialize;
use serde::Serialize;

use crate::fs::project_rel_path::ProjectRelativePath;

/// An interned `ProjectRelativePath`. This is `Copy` and wraps an
/// `IForwardRelativePath` with the project-relative semantic.
///
/// See `IForwardRelativePath` for details on the interning strategy.
#[derive(
    Copy,
    Clone,
    Dupe,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Allocative,
    Serialize,
    Deserialize,
    Pagable
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct IProjectRelativePath {
    #[pagable(flatten_serde)]
    path: IForwardRelativePath,
}

impl IProjectRelativePath {
    #[inline]
    pub const fn empty() -> Self {
        IProjectRelativePath {
            path: IForwardRelativePath::empty(),
        }
    }

    #[inline]
    pub fn intern_directly(path: &ProjectRelativePath) -> Self {
        IProjectRelativePath {
            path: IForwardRelativePath::intern_directly(path.as_forward_relative_path()),
        }
    }

    pub fn intern_concat_directly<'a, I>(parts: I) -> Self
    where
        I: IntoIterator<Item = &'a ForwardRelativePath>,
        I::IntoIter: Clone,
    {
        IProjectRelativePath {
            path: IForwardRelativePath::intern_concat_directly(parts),
        }
    }

    pub fn join(self, path: &ForwardRelativePath) -> Self {
        Self::intern_concat_directly([self.as_forward_relative_path(), path])
    }

    #[inline]
    pub fn as_str(self) -> &'static str {
        self.path.as_str()
    }

    #[inline]
    pub fn as_project_relative_path(self) -> &'static ProjectRelativePath {
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

    #[inline]
    pub fn parent(self) -> Option<Self> {
        self.path.parent().map(|p| Self { path: p })
    }

    #[inline]
    pub fn file_name(self) -> Option<&'static FileName> {
        self.path.file_name()
    }

    /// The parent and the final component, unless the path is empty.
    #[inline]
    pub fn split_last(self) -> Option<(Self, &'static FileName)> {
        self.path
            .split_last()
            .map(|(parent, name)| (Self { path: parent }, name))
    }
}

impl Deref for IProjectRelativePath {
    type Target = ProjectRelativePath;

    #[inline]
    fn deref(&self) -> &ProjectRelativePath {
        self.as_project_relative_path()
    }
}

impl AsRef<ProjectRelativePath> for IProjectRelativePath {
    #[inline]
    fn as_ref(&self) -> &ProjectRelativePath {
        self.as_project_relative_path()
    }
}

impl AsRef<ForwardRelativePath> for IProjectRelativePath {
    #[inline]
    fn as_ref(&self) -> &ForwardRelativePath {
        self.as_forward_relative_path()
    }
}

impl fmt::Display for IProjectRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_project_relative_path(), f)
    }
}

impl fmt::Debug for IProjectRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "IProjectRelativePath({:?})", self.as_str())
    }
}

impl From<IForwardRelativePath> for IProjectRelativePath {
    #[inline]
    fn from(path: IForwardRelativePath) -> Self {
        IProjectRelativePath { path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let p = IProjectRelativePath::empty();
        assert!(p.is_empty());
        assert_eq!("", p.as_str());
    }

    #[test]
    fn test_intern_and_equality() {
        let a = IProjectRelativePath::intern_directly(ProjectRelativePath::new("foo/bar").unwrap());
        let b = IProjectRelativePath::intern_directly(ProjectRelativePath::new("foo/bar").unwrap());
        assert_eq!(a, b);
        assert_eq!(a.as_str().as_ptr(), b.as_str().as_ptr());
    }

    #[test]
    fn test_deref_to_project_relative_path() {
        let p = IProjectRelativePath::intern_directly(ProjectRelativePath::new("foo/bar").unwrap());
        let prp: &ProjectRelativePath = &p;
        assert_eq!("foo/bar", prp.as_str());
    }

    #[test]
    fn test_display_debug() {
        let p = IProjectRelativePath::intern_directly(ProjectRelativePath::new("foo/bar").unwrap());
        assert_eq!("foo/bar", format!("{p}"));
        assert_eq!("IProjectRelativePath(\"foo/bar\")", format!("{p:?}"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let p = IProjectRelativePath::intern_directly(ProjectRelativePath::new("foo/bar").unwrap());
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!("\"foo/bar\"", json);
        let deserialized: IProjectRelativePath = serde_json::from_str(&json).unwrap();
        assert_eq!(p, deserialized);
    }

    #[test]
    fn test_inner_forward_path() {
        let p = IProjectRelativePath::intern_directly(ProjectRelativePath::new("foo/bar").unwrap());
        let inner = p.inner();
        assert_eq!("foo/bar", inner.as_str());
    }
}
