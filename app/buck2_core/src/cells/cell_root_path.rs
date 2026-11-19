/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Borrow;
use std::ops::Deref;

use allocative::Allocative;
use pagable::Pagable;
use ref_cast::RefCast;

use crate::cells::paths::CellRelativePath;
use crate::fs::project_rel_path::ProjectRelativePath;
use crate::fs::project_rel_path::ProjectRelativePathBuf;

/// Path to the cell root.
#[derive(RefCast, Debug, Hash, Eq, PartialEq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct CellRootPath(ProjectRelativePath);

impl CellRootPath {
    /// Constructor. Does not check the path is a valid cell root.
    pub fn new(path: &ProjectRelativePath) -> &CellRootPath {
        CellRootPath::ref_cast(path)
    }

    pub fn testing_new(path: &str) -> &CellRootPath {
        CellRootPath::new(ProjectRelativePath::new(path).unwrap())
    }

    /// Project relative path to the cell root.
    pub fn as_project_relative_path(&self) -> &ProjectRelativePath {
        &self.0
    }

    /// Join cell path and cell-relative path.
    pub fn join(&self, path: &CellRelativePath) -> ProjectRelativePathBuf {
        self.0.join(path)
    }

    /// To owned.
    pub fn to_buf(&self) -> CellRootPathBuf {
        CellRootPathBuf(self.0.to_buf())
    }

    #[inline]
    pub fn is_repo_root(&self) -> bool {
        self.0.is_empty()
    }
}

impl Deref for CellRootPath {
    type Target = ProjectRelativePath;

    fn deref(&self) -> &ProjectRelativePath {
        &self.0
    }
}

impl AsRef<ProjectRelativePath> for CellRootPath {
    fn as_ref(&self) -> &ProjectRelativePath {
        &self.0
    }
}

/// Path to the cell root.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    Allocative,
    Pagable
)]
pub struct CellRootPathBuf(ProjectRelativePathBuf);

impl CellRootPathBuf {
    /// Constructor. Does not check the path is a valid cell root.
    pub fn new(path: ProjectRelativePathBuf) -> Self {
        CellRootPathBuf(path)
    }

    pub fn testing_new(path: &str) -> CellRootPathBuf {
        CellRootPathBuf::new(ProjectRelativePathBuf::testing_new(path))
    }

    pub fn as_path(&self) -> &CellRootPath {
        CellRootPath::new(&self.0)
    }
}

impl Deref for CellRootPathBuf {
    type Target = CellRootPath;

    fn deref(&self) -> &CellRootPath {
        CellRootPath::new(&self.0)
    }
}

impl Borrow<CellRootPath> for CellRootPathBuf {
    fn borrow(&self) -> &CellRootPath {
        self.deref()
    }
}
