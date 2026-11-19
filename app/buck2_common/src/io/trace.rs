/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::any::Any;
use std::sync::Arc;

use allocative::Allocative;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckDashMap;
use buck2_hash::BuckDashSet;

use crate::file_ops::metadata::RawPathMetadata;
use crate::file_ops::metadata::RawSymlink;
use crate::io::IoProvider;
use crate::io::ReadDirOutcome;

#[derive(Allocative, Debug, Hash, PartialEq, Eq, Clone)]
pub struct Symlink {
    pub at: ProjectRelativePathBuf,
    pub to: RawSymlink<ProjectRelativePathBuf>,
}

/// The value recorded with a buck-out entry of the trace, so that exporting the trace can ask the
/// materializer for the entry together with the value it was produced with. The concrete type is
/// `buck2_execute`'s `ArtifactValue`, which lives above this crate; readers downcast through
/// `as_any`.
pub trait TracedBuckOutValue: Any + Send + Sync + Allocative {
    fn as_any(&self) -> &dyn Any;
}

#[derive(Allocative)]
pub struct Trace {
    pub project_entries: BuckDashSet<ProjectRelativePathBuf>,
    pub buck_out_entries: BuckDashMap<ProjectRelativePathBuf, Arc<dyn TracedBuckOutValue>>,
    pub external_entries: BuckDashSet<AbsNormPathBuf>,
    pub symlinks: BuckDashSet<Symlink>,
}

impl Trace {
    pub fn new() -> Self {
        Self {
            project_entries: BuckDashSet::default(),
            buck_out_entries: BuckDashMap::default(),
            external_entries: BuckDashSet::default(),
            symlinks: BuckDashSet::default(),
        }
    }

    /// Provides an owned view of the underlying trace state.
    pub fn project_entries(&self) -> Vec<ProjectRelativePathBuf> {
        self.project_entries
            .iter()
            .map(|path| path.key().to_buf())
            .collect()
    }

    pub fn buck_out_entries(&self) -> Vec<(ProjectRelativePathBuf, Arc<dyn TracedBuckOutValue>)> {
        self.buck_out_entries
            .iter()
            .map(|entry| (entry.key().to_buf(), entry.value().clone()))
            .collect()
    }

    pub fn external_entries(&self) -> Vec<AbsNormPathBuf> {
        self.external_entries
            .iter()
            .map(|path| path.key().to_owned())
            .collect()
    }
}

#[derive(Allocative)]
pub struct TracingIoProvider {
    io: Box<dyn IoProvider>,
    trace: Trace,
}

impl TracingIoProvider {
    pub fn new(io: Box<dyn IoProvider>) -> Self {
        Self {
            io,
            trace: Trace::new(),
        }
    }

    pub fn from_io(io: &dyn IoProvider) -> Option<&Self> {
        io.as_any().downcast_ref::<Self>()
    }

    pub fn add_project_path(&self, path: ProjectRelativePathBuf) {
        self.trace.project_entries.insert(path);
    }

    pub fn add_buck_out_entry(
        &self,
        entry: ProjectRelativePathBuf,
        value: Arc<dyn TracedBuckOutValue>,
    ) {
        self.trace.buck_out_entries.insert(entry, value);
    }

    pub fn add_external_path(&self, path: AbsNormPathBuf) {
        self.trace.external_entries.insert(path);
    }

    pub fn add_symlink(&self, link: Symlink) {
        self.trace.symlinks.insert(link);
    }

    pub fn trace(&self) -> &Trace {
        &self.trace
    }
}

#[async_trait::async_trait]
impl IoProvider for TracingIoProvider {
    /// Combination of read_file_if_exists from underlying fs struct and reading
    /// the metadata. This is done so we get accurate path classification (e.g.
    /// if the path is a real file/dir or a symlink pointing somewhere else).
    ///
    /// This makes code working with the exported I/O manifest much easier to
    /// work with at the expense of some additional I/O during tracing builds.
    async fn read_file_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<String>> {
        let res = self.io.read_file_if_exists_impl(path).await?;
        if res.is_some() {
            self.add_project_path(path.to_buf());
        }
        Ok(res)
    }

    /// Combination of read_file_if_exists from underlying fs struct and reading
    /// the metadata. This is done so we get accurate path classification (e.g.
    /// if the path is a real file/dir or a symlink pointing somewhere else).
    ///
    /// This makes code working with the exported I/O manifest much easier to
    /// work with at the expense of some additional I/O during tracing builds.
    async fn read_dir_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<ReadDirOutcome> {
        let entries = self.io.read_dir_impl(path).await?.into_entries();
        self.add_project_path(path.to_buf());
        for entry in entries.iter() {
            self.add_project_path(
                path.as_project_relative_path()
                    .join(ForwardRelativePath::unchecked_new(&entry.file_name)),
            );
        }

        Ok(ReadDirOutcome::Entries(entries))
    }

    async fn read_path_metadata_if_exists_impl(
        &self,
        path: IProjectRelativePath,
    ) -> buck2_error::Result<Option<RawPathMetadata<ProjectRelativePathBuf>>> {
        let res = self.io.read_path_metadata_if_exists_impl(path).await?;
        match &res {
            Some(RawPathMetadata::File(_)) | Some(RawPathMetadata::Directory) => {
                self.add_project_path(path.to_buf());
            }
            Some(RawPathMetadata::Symlink { at, to }) => {
                self.add_symlink(Symlink {
                    at: at.clone(),
                    to: to.clone(),
                });
            }
            _ => {}
        }

        Ok(res)
    }

    async fn settle(&self) -> buck2_error::Result<()> {
        self.io.settle().await
    }

    fn name(&self) -> &'static str {
        self.io.name()
    }

    async fn eden_version(&self) -> buck2_error::Result<Option<String>> {
        self.io.eden_version().await
    }

    async fn verify_eden_identity(&self) -> buck2_error::Result<()> {
        self.io.verify_eden_identity().await
    }

    fn project_root(&self) -> &ProjectRoot {
        self.io.project_root()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn is_eden_repo(&self) -> bool {
        self.io.is_eden_repo()
    }
}
