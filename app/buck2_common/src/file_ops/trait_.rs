/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use async_trait::async_trait;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_fs::paths::file_name::FileNameBuf;
use dice::LinearRecomputeDiceComputations;
use dupe::Dupe;

use crate::file_ops::dice::DiceFileComputations;
use crate::file_ops::metadata::RawPathMetadata;
use crate::file_ops::metadata::ReadDirOutput;
use crate::ignores::file_ignores::FileIgnoreResult;

#[async_trait]
pub trait FileOps: Send + Sync {
    async fn read_file_if_exists(&self, path: CellPath) -> buck2_error::Result<Option<String>>;

    /// Return the list of file outputs, sorted.
    async fn read_dir(&self, path: CellPath) -> buck2_error::Result<ReadDirOutput>;

    async fn is_ignored(&self, path: CellPath) -> buck2_error::Result<FileIgnoreResult>;

    async fn read_path_metadata_if_exists(
        &self,
        path: CellPath,
    ) -> buck2_error::Result<Option<RawPathMetadata>>;

    async fn buildfiles<'a>(&self, cell: CellName) -> buck2_error::Result<Arc<[FileNameBuf]>>;
}

/// A wrapper around DiceComputations for places that want to interact with a dyn FileOps.
///
/// In general, it's better to use DiceFileComputations directly.
pub struct DiceFileOps<'c, 'd>(pub LinearRecomputeDiceComputations<'c, 'd>);

#[async_trait]
impl FileOps for DiceFileOps<'_, '_> {
    async fn read_file_if_exists(&self, path: CellPath) -> buck2_error::Result<Option<String>> {
        DiceFileComputations::read_file_if_exists(&mut self.0.get(), path).await
    }

    async fn read_dir(&self, path: CellPath) -> buck2_error::Result<ReadDirOutput> {
        DiceFileComputations::read_dir(&mut self.0.get(), path).await
    }

    async fn read_path_metadata_if_exists(
        &self,
        path: CellPath,
    ) -> buck2_error::Result<Option<RawPathMetadata>> {
        DiceFileComputations::read_path_metadata_if_exists(&mut self.0.get(), path).await
    }

    async fn is_ignored(&self, path: CellPath) -> buck2_error::Result<FileIgnoreResult> {
        DiceFileComputations::is_ignored(&mut self.0.get(), path).await
    }

    async fn buildfiles<'a>(&self, cell: CellName) -> buck2_error::Result<Arc<[FileNameBuf]>> {
        DiceFileComputations::buildfiles(&mut self.0.get(), cell)
            .await
            .map(Dupe::dupe)
    }
}
