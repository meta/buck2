/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// Internal stable rustc still treats `assert_matches` as unstable; OSS nightly has stabilized it
// and denies the now-redundant feature gate.
#![allow(stable_features)]
#![feature(once_cell_try)]

use std::sync::Arc;

use async_trait::async_trait;
use buck2_common::dice::data::HasIoProvider;
use buck2_common::file_ops::delegate::FileOpsDelegate;
use buck2_common::file_ops::metadata::RawPathMetadata;
use buck2_core::cells::cell_root_path::CellRootPath;
use buck2_core::cells::external::ExternalCellOrigin;
use buck2_core::cells::name::CellName;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;
use buck2_execute::execute::blocking::HasBlockingExecutor;
use buck2_execute::execute::readonly_outputs::make_output_paths_writable;
use dice::DiceComputations;
use dupe::Dupe;

mod bundled;
mod git;

struct ConcreteExternalCellsImpl;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
enum ExternalCellsError {
    #[error("Tried to expand external cell to `{0}`, but that directory already contains data!")]
    ExpandDataAlreadyPresent(IProjectRelativePath),
}

#[async_trait]
impl buck2_common::external_cells::ExternalCellsImpl for ConcreteExternalCellsImpl {
    async fn get_file_ops_delegate(
        &self,
        ctx: &mut DiceComputations<'_>,
        cell_name: CellName,
        origin: ExternalCellOrigin,
    ) -> buck2_error::Result<Arc<dyn FileOpsDelegate>> {
        match origin {
            ExternalCellOrigin::Bundled(cell_name) => {
                Ok(bundled::get_file_ops_delegate(ctx, cell_name).await? as _)
            }
            ExternalCellOrigin::Git(setup) => {
                Ok(git::get_file_ops_delegate(ctx, cell_name, setup).await? as _)
            }
        }
    }

    fn check_bundled_cell_exists(&self, cell_name: CellName) -> buck2_error::Result<()> {
        bundled::find_bundled_data(cell_name).map(|_| ())
    }

    async fn expand(
        &self,
        ctx: &mut DiceComputations<'_>,
        cell: CellName,
        origin: ExternalCellOrigin,
        path: &CellRootPath,
    ) -> buck2_error::Result<()> {
        let dest_path = IProjectRelativePath::intern_directly(path.as_project_relative_path());
        let io = ctx.global_data().get_io_provider();

        // Make sure we're not about to overwrite existing data
        match io.read_path_metadata_if_exists(dest_path).await? {
            None => (),
            Some(RawPathMetadata::Directory) => {
                let data = io.read_dir(dest_path).await?.into_entries();
                if !data.is_empty() {
                    return Err(ExternalCellsError::ExpandDataAlreadyPresent(dest_path).into());
                }
            }
            Some(_) => {
                return Err(ExternalCellsError::ExpandDataAlreadyPresent(dest_path).into());
            }
        }

        // Materialize the whole cell, and then copy it into the repository.
        //
        // FIXME(JakobDegen): Ideally we'd be able to ask the materializer to just make a copy
        // without doing the actual materialization. However, that's not currently possible without
        // it resulting in the materializer tracking paths in the repo, so this will have to do for
        // now.
        let (materialized_path, lease) = match origin {
            ExternalCellOrigin::Bundled(cell) => bundled::materialize_all(ctx, cell).await?,
            ExternalCellOrigin::Git(setup) => git::materialize_all(ctx, cell, setup).await?,
        };

        let fs = io.project_root().dupe();
        let copied = fs.copy(&materialized_path, dest_path);
        // The copy read the cell under the lease.
        drop(lease);
        copied?;
        // The copy carries the materialized files' modes, read-only under
        // `buck2.readonly_outputs`, and the expanded cell is the user's to edit.
        ctx.get_blocking_executor()
            .execute_io_inline(move || make_output_paths_writable(&fs, &[dest_path.to_buf()]))
            .await
    }
}

pub fn init_late_bindings() {
    buck2_common::external_cells::EXTERNAL_CELLS_IMPL.init(&ConcreteExternalCellsImpl);
}
