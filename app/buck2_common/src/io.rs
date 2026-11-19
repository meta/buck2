/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod fs;
pub mod trace;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_events::dispatch::console_message;

use crate::file_ops::metadata::RawDirEntry;
use crate::file_ops::metadata::RawPathMetadata;
use crate::ignores::file_ignores::FileIgnoreReason;

/// Tells the user about directory entries whose names buck2 cannot represent (a `\\` on Linux,
/// say). Providers leave such entries out of their listings and call this with them.
pub fn report_invalid_file_names(dir: &ProjectRelativePath, names: &[String]) {
    for name in names {
        console_message(format!(
            "File name `{name}` in `{dir}` is not valid, the entry is ignored"
        ));
    }
}

pub enum ReadDirOutcome {
    Entries(Vec<RawDirEntry>),
    /// ACL-restricted path on an Eden repo -- treat as empty directory.
    EdenPermissionDenied,
}

impl ReadDirOutcome {
    pub fn into_entries(self) -> Vec<RawDirEntry> {
        match self {
            Self::Entries(entries) => entries,
            Self::EdenPermissionDenied => vec![],
        }
    }
}

#[derive(Debug, Allocative, buck2_error::Error)]
#[buck2(tag = Input)]
pub enum ReadDirError {
    #[error("Directory `{path}` does not exist")]
    DirectoryDoesNotExist {
        path: CellPath,
        suggestion: DirectoryDoesNotExistSuggestion,
    },
    #[error("Directory `{0}` is ignored ({})", .1.describe())]
    DirectoryIsIgnored(CellPath, FileIgnoreReason),
    #[error("Path `{0}` is `{1}`, not a directory")]
    NotADirectory(CellPath, String),
    /// `at` is the symlink: the path itself, or a prefix of it.
    #[error("{}", symlink_message(path, at))]
    Symlink { path: CellPath, at: CellPath },
    #[error(transparent)]
    Error(buck2_error::Error),
}

fn symlink_message(path: &CellPath, at: &CellPath) -> String {
    if path == at {
        format!("`{path}` is a symlink, which buck2 does not follow")
    } else {
        format!("`{path}` is behind the symlink `{at}`, which buck2 does not follow")
    }
}

#[derive(Debug, Allocative)]
pub enum DirectoryDoesNotExistSuggestion {
    Cell(Vec<String>),
    Typo(String),
    NoSuggestion,
}

impl From<buck2_error::Error> for ReadDirError {
    fn from(value: buck2_error::Error) -> Self {
        Self::Error(value)
    }
}

/// Reads of the project's files, by exact path.
///
/// Callers establish, before asking, that every parent component of the path is a plain
/// directory, spelled as given; see `file_ops::path_check`. Providers may therefore answer by
/// looking at the path itself and need not detect symlinks or case differences above it.
#[async_trait]
pub trait IoProvider: Allocative + Send + Sync {
    async fn read_file_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<String>>;

    async fn read_dir_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<ReadDirOutcome>;

    async fn read_path_metadata_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<RawPathMetadata<ProjectRelativePathBuf>>>;

    /// Request that this I/O provider be up to date with whatever I/O operations the user might
    /// have done until this point.
    async fn settle(&self) -> buck2_error::Result<()>;

    fn name(&self) -> &'static str;

    /// Returns the Eden version of the underlying system of the IoProvider, if available.
    async fn eden_version(&self) -> buck2_error::Result<Option<String>>;

    /// Verify that the Eden daemon backing this I/O provider has not restarted since the
    /// provider was created. A restart invalidates cached state and file handles, so commands
    /// should fail fast rather than silently hang on them. Providers that don't cache state
    /// against an Eden daemon have nothing to verify.
    async fn verify_eden_identity(&self) -> buck2_error::Result<()> {
        Ok(())
    }

    fn project_root(&self) -> &ProjectRoot;

    fn as_any(&self) -> &dyn std::any::Any;

    fn is_eden_repo(&self) -> bool;
}

impl dyn IoProvider + '_ {
    pub async fn read_file_if_exists(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<String>> {
        self.read_file_if_exists_impl(path)
            .await
            .tag(ErrorTag::IoSource)
    }

    pub async fn read_dir(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<ReadDirOutcome> {
        self.read_dir_impl(path).await.tag(ErrorTag::IoSource)
    }

    pub async fn read_path_metadata_if_exists(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<RawPathMetadata<ProjectRelativePathBuf>>> {
        self.read_path_metadata_if_exists_impl(path)
            .await
            .tag(ErrorTag::IoSource)
    }
}
