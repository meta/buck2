/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use crate::IoResultExt;
use crate::fs_util;
use crate::paths::abs_norm_path::AbsNormPath;
use crate::paths::abs_norm_path::AbsNormPathBuf;
use crate::paths::abs_path::AbsPathBuf;

/// Client working directory.
///
/// Can be different from process working directory if process changes the directory.
/// So relative paths should be resolved against this.
#[derive(Clone, Debug, derive_more::Display)]
#[display("{}", path)]
pub struct AbsWorkingDir {
    path: AbsNormPathBuf,
}

impl AbsWorkingDir {
    pub fn unchecked_new(path: AbsNormPathBuf) -> AbsWorkingDir {
        AbsWorkingDir { path }
    }

    pub fn current_dir() -> buck2_error::Result<AbsWorkingDir> {
        let current_dir = fs_util::current_dir()?;

        #[derive(Debug, buck2_error::Error)]
        #[buck2(tier0)]
        enum CurrentDirError {
            #[error("std::env::current_dir returns non-canonical path: `{}` -> `{}`", _0.display(), _1.display())]
            NotCanonical(AbsPathBuf, AbsNormPathBuf),
        }

        // `current_dir` seems to return canonical path everywhere except Windows,
        // but may return non-canonical path on Windows:
        // https://fb.workplace.com/groups/buck2windows/posts/754618429743405
        let current_dir_canonical = fs_util::canonicalize(&current_dir).categorize_internal()?;

        if current_dir.as_path() != current_dir_canonical.as_path() {
            if !cfg!(windows) {
                return Err(
                    CurrentDirError::NotCanonical(current_dir, current_dir_canonical).into(),
                );
            }
        }

        Ok(AbsWorkingDir::unchecked_new(current_dir_canonical))
    }

    pub fn resolve(&self, path: &Path) -> AbsPathBuf {
        self.path.as_abs_path().join(path)
    }

    pub fn path(&self) -> &AbsNormPath {
        &self.path
    }

    pub fn into_abs_norm_path_buf(self) -> AbsNormPathBuf {
        self.path
    }
}
