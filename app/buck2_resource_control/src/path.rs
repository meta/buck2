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

use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;

/// A path identifying a cgroup.
///
/// This path includes the cgroup fs mount point, typically `/sys/fs/cgroup`.
#[derive(
    derive_more::Display,
    Debug,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    ref_cast::RefCastCustom
)]
#[display("{}", _0.display())]
#[repr(transparent)]
pub struct CgroupPath(AbsNormPath);

impl CgroupPath {
    pub fn new(path: &AbsNormPath) -> &Self {
        Self::from_abs_path(path)
    }

    #[ref_cast::ref_cast_custom]
    fn from_abs_path(path: &AbsNormPath) -> &Self;

    pub fn to_buf(&self) -> CgroupPathBuf {
        CgroupPathBuf(self.0.to_buf())
    }

    pub fn join(&self, path: &ForwardRelativePath) -> CgroupPathBuf {
        CgroupPathBuf(self.0.join(path))
    }

    pub fn parent(&self) -> Option<&CgroupPath> {
        if &self.0 == AbsNormPath::new("/sys/fs/cgroup").unwrap() {
            None
        } else {
            Some(Self::from_abs_path(self.0.parent().unwrap()))
        }
    }
}

/// Owned version of [`CgroupPath`].
#[derive(
    derive_more::Display,
    Debug,
    Clone,
    Hash,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    allocative::Allocative
)]
#[display("{}", _0.display())]
pub struct CgroupPathBuf(AbsNormPathBuf);

impl CgroupPathBuf {
    pub fn new(path: AbsNormPathBuf) -> Self {
        Self(path)
    }

    pub fn new_in_cgroup_fs(path: &AbsNormPath) -> Self {
        // Can't use .join() since the second part is absolute too
        Self(AbsNormPathBuf::from(format!("/sys/fs/cgroup{path}")).unwrap())
    }
}

impl Deref for CgroupPath {
    type Target = AbsNormPath;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for CgroupPathBuf {
    type Target = CgroupPath;

    fn deref(&self) -> &Self::Target {
        CgroupPath::from_abs_path(&self.0)
    }
}

impl Borrow<CgroupPath> for CgroupPathBuf {
    fn borrow(&self) -> &CgroupPath {
        self
    }
}

#[cfg(test)]
mod tests {
    use buck2_fs::paths::abs_norm_path::AbsNormPath;

    use crate::path::CgroupPath;

    #[test]
    fn test_parent() {
        if !cfg!(unix) {
            return;
        }
        let p = CgroupPath::new(AbsNormPath::new("/sys/fs/cgroup/cg1/cg2").unwrap());
        assert_eq!(&p.parent().unwrap().to_string(), "/sys/fs/cgroup/cg1");
        assert_eq!(
            &p.parent().unwrap().parent().unwrap().to_string(),
            "/sys/fs/cgroup",
        );
        assert!(p.parent().unwrap().parent().unwrap().parent().is_none());
    }
}
