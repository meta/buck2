/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;

use crate::execute::blocking::IoRequest;

/// IoRequest we dispatch to the blocking executor to clear output paths.
pub struct CleanOutputPaths {
    pub paths: Vec<ProjectRelativePathBuf>,
}

impl CleanOutputPaths {
    pub fn clean<'a>(
        paths: impl IntoIterator<Item = &'a ProjectRelativePath>,
        fs: &'a ProjectRoot,
    ) -> buck2_error::Result<()> {
        for path in paths {
            cleanup_path(fs, path)
                .with_buck_error_context(|| format!("Error cleaning up output path `{path}`"))?;
        }
        Ok(())
    }
}

/// Tags a failure to remove or replace a file that another process holds open
/// (Windows sharing semantics) as an environment error.
#[cfg(unix)]
pub fn tag_file_busy_error(error: buck2_error::Error) -> buck2_error::Error {
    error
}

#[cfg(windows)]
pub fn tag_file_busy_error(error: buck2_error::Error) -> buck2_error::Error {
    use buck2_error::ErrorTag;
    if error.has_tag(ErrorTag::IoWindowsSharingViolation)
        | error.has_tag(ErrorTag::IoPermissionDenied)
    {
        error
            .tag([ErrorTag::IoMaterializerFileBusy])
            .context("Binary being executed, please close the process first")
    } else {
        error
    }
}

/// Clears `path` for an output to be written there: whatever is at `path` goes, and so does a
/// stale file or symlink sitting where one of its parent directories has to be (T85589819).
///
/// The parents are examined from the top down, each with `lstat`, and the walk stops at the first
/// that is not a directory: below a file nothing exists, and below a symlink `path` would resolve
/// to somewhere else, where removing it would destroy content that is not ours, such as the
/// source tree a symlink artifact from an earlier build points at. The first component, buck-out
/// itself, is trusted whatever it is: a redirected buck-out is a symlink or a mount. An `lstat`
/// that fails for any reason but absence is an error, since below a parent of unknown kind
/// nothing may be removed.
#[tracing::instrument(level = "debug", skip(fs), fields(path = %path))]
pub fn cleanup_path(fs: &ProjectRoot, path: &ProjectRelativePath) -> buck2_error::Result<()> {
    let mut parent = fs.root().to_buf();
    for (depth, name) in path
        .parent()
        .into_iter()
        .flat_map(|parent| parent.iter())
        .enumerate()
    {
        parent.push(name);
        if depth == 0 {
            continue;
        }
        match fs_util::symlink_metadata_if_exists(&parent)? {
            Some(metadata) if metadata.is_dir() => {}
            None => return Ok(()),
            Some(_) => return clear_stale_parent(&parent),
        }
    }
    fs_util::remove_all(fs.resolve(path))
        .categorize_internal()
        .map_err(tag_file_busy_error)
}

/// Removes a stale file or symlink where a parent directory has to go. Another output under that
/// directory may be doing the same at this moment: if by the time we get to it the file is gone,
/// or a directory has taken its place, that other output made room already. Only `unlink` is
/// used, which a directory never yields to, so racing with the sibling's `mkdir` cannot remove
/// what the sibling put there.
fn clear_stale_parent(parent: &AbsNormPath) -> buck2_error::Result<()> {
    let Err(e) = fs_util::remove_file_harder(parent) else {
        return Ok(());
    };
    match fs_util::symlink_metadata_if_exists(parent) {
        Ok(None) => Ok(()),
        Ok(Some(metadata)) if metadata.is_dir() => Ok(()),
        _ => Err(e).categorize_internal().map_err(tag_file_busy_error),
    }
}

impl IoRequest for CleanOutputPaths {
    fn execute(self: Box<Self>, project_fs: &ProjectRoot) -> buck2_error::Result<()> {
        Self::clean(self.paths.iter().map(AsRef::as_ref), project_fs)
    }
}

#[cfg(test)]
mod tests {
    use buck2_core::fs::project::ProjectRootTemp;
    use buck2_core::fs::project_rel_path::ProjectRelativePath;
    #[cfg(unix)]
    use buck2_fs::fs_util::symlink_metadata_if_exists;
    use buck2_fs::fs_util::uncategorized as fs_util;

    use super::*;

    fn rel(s: &str) -> &ProjectRelativePath {
        ProjectRelativePath::unchecked_new(s)
    }

    #[cfg(unix)]
    #[test]
    fn test_a_symlinked_parent_is_unlinked_and_not_followed() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        fs.write_file(rel("elsewhere/b"), "keep", false)?;
        fs_util::create_dir_all(fs.resolve(rel("out/gen")))?;
        fs_util::symlink(
            fs.resolve(rel("elsewhere")),
            fs.resolve(rel("out/gen/link")),
        )?;
        cleanup_path(fs, rel("out/gen/link/b"))?;
        assert!(
            symlink_metadata_if_exists(fs.resolve(rel("out/gen/link")))?.is_none(),
            "the link goes"
        );
        assert!(
            fs.resolve(rel("elsewhere/b")).exists(),
            "what it pointed at stays"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn test_a_redirected_first_component_is_followed() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        fs.write_file(rel("real/x/y"), "old", false)?;
        fs_util::symlink(fs.resolve(rel("real")), fs.resolve(rel("out")))?;
        cleanup_path(fs, rel("out/x/y"))?;
        assert!(!fs.resolve(rel("real/x/y")).exists());
        assert!(
            symlink_metadata_if_exists(fs.resolve(rel("out")))?
                .is_some_and(|metadata| metadata.is_symlink())
        );
        Ok(())
    }

    #[test]
    fn test_a_stale_parent_a_sibling_already_cleared_is_fine() -> buck2_error::Result<()> {
        let temp = ProjectRootTemp::new()?;
        let fs = temp.path();
        fs_util::create_dir_all(fs.resolve(rel("out/dir")))?;
        clear_stale_parent(&fs.resolve(rel("out/dir")))?;
        assert!(fs.resolve(rel("out/dir")).is_dir());
        clear_stale_parent(&fs.resolve(rel("out/gone")))?;
        Ok(())
    }
}
