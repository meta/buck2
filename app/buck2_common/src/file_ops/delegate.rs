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

use allocative::Allocative;
use async_trait::async_trait;
use buck2_core::cells::CellResolver;
use buck2_core::cells::i_cell_relative_path::ICellRelativePath;
use buck2_core::cells::name::CellName;
use buck2_fs::paths::file_name::FileName;
use buck2_util::arc_str::PackedStringSlice;
use dice::DiceComputations;
use dupe::Dupe;

use crate::external_cells::EXTERNAL_CELLS_IMPL;
use crate::file_ops::metadata::FileType;
use crate::file_ops::metadata::RawPathMetadata;

/// Reads for a cell whose files the `IoProvider` does not serve, that is, an external cell.
#[async_trait]
pub trait FileOpsDelegate: Allocative + Send + Sync {
    async fn read_file_if_exists(
        &self,
        ctx: &mut DiceComputations<'_>,
        path: ICellRelativePath,
    ) -> buck2_error::Result<Option<String>>;

    /// Return the list of file outputs, sorted by file name.
    async fn read_dir(
        &self,
        ctx: &mut DiceComputations<'_>,
        path: ICellRelativePath,
    ) -> buck2_error::Result<PackedStringSlice<FileName, FileType>>;

    async fn read_path_metadata_if_exists(
        &self,
        ctx: &mut DiceComputations<'_>,
        path: ICellRelativePath,
    ) -> buck2_error::Result<Option<RawPathMetadata>>;

    /// Whether anything is at the path, spelled exactly like that.
    ///
    /// The default implementation assumes that `read_path_metadata_if_exists` is already exact
    /// about spelling; a delegate for which it is not must override this.
    async fn exists(
        &self,
        ctx: &mut DiceComputations<'_>,
        path: ICellRelativePath,
    ) -> buck2_error::Result<bool> {
        let metadata = self.read_path_metadata_if_exists(ctx, path).await?;
        Ok(metadata.is_some())
    }
}

/// The delegate serving `cell`, if it is an external cell.
pub(crate) async fn external_cell_delegate(
    ctx: &mut DiceComputations<'_>,
    cells: &CellResolver,
    cell: CellName,
) -> buck2_error::Result<Option<Arc<dyn FileOpsDelegate>>> {
    match cells.get(cell)?.external() {
        Some(origin) => Ok(Some(
            EXTERNAL_CELLS_IMPL
                .get()?
                .get_file_ops_delegate(ctx, cell, origin.dupe())
                .await?,
        )),
        None => Ok(None),
    }
}
