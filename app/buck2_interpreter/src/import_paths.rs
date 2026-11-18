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
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::paths::CellRelativePathBuf;
use buck2_core::pattern::parse_package::maybe_parse_cell_path;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(PartialEq, Allocative, Pagable)]
pub struct ImplicitImportPaths {
    pub root_import: Option<ImportPath>,
}

impl ImplicitImportPaths {
    pub fn parse(
        mut config: impl LegacyBuckConfigView,
        cell_name: BuildFileCell,
        cell_alias_resolver: &CellAliasResolver,
    ) -> buck2_error::Result<ImplicitImportPaths> {
        // Oddly, the root import is defined to use a more path-like representation than
        // normal imports. e.g. it uses `cell//path/to/file.bzl` instead of
        // `cell//path/to:file.bzl`.
        let root_import = config
            .get(BuckconfigKeyRef {
                section: "buildfile",
                property: "includes",
            })?
            .map(|i| {
                let path = match maybe_parse_cell_path(&i, cell_alias_resolver)? {
                    Some(cell_path) => cell_path,
                    None => {
                        let path = CellRelativePathBuf::try_from(i.to_string())?;
                        CellPath::new(cell_alias_resolver.resolve_self(), &path)
                    }
                };

                // root imports are only going to be used by a top-level module in the cell they
                // are defined, so we can set the build_file_cell early.
                ImportPath::new_with_build_file_cells(path, cell_name)
            })
            .map_or(Ok(None), |e: buck2_error::Result<ImportPath>| e.map(Some))?;
        Ok(ImplicitImportPaths { root_import })
    }

    pub fn root_import(&self) -> Option<&ImportPath> {
        self.root_import.as_ref()
    }
}

#[async_trait]
pub trait HasImportPaths<'d> {
    async fn import_paths_for_cell(
        &mut self,
        cell_name: BuildFileCell,
    ) -> buck2_error::Result<&'d Arc<ImplicitImportPaths>>;
}

#[async_trait]
impl<'d> HasImportPaths<'d> for DiceComputations<'d> {
    async fn import_paths_for_cell(
        &mut self,
        cell_name: BuildFileCell,
    ) -> buck2_error::Result<&'d Arc<ImplicitImportPaths>> {
        #[derive(
            Debug,
            Eq,
            PartialEq,
            Hash,
            Clone,
            derive_more::Display,
            Allocative,
            Pagable
        )]
        #[display("{}", cell_name)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct ImportPathsKey {
            cell_name: BuildFileCell,
        }

        #[async_trait]
        impl Key for ImportPathsKey {
            type Value = buck2_error::Result<Arc<ImplicitImportPaths>>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                let config = ctx.get_legacy_config_on_dice(self.cell_name.name()).await?;
                let cell_alias_resolver =
                    ctx.get_cell_alias_resolver(self.cell_name.name()).await?;

                Ok(Arc::new(ImplicitImportPaths::parse(
                    config.view(ctx),
                    self.cell_name,
                    cell_alias_resolver,
                )?))
            }

            fn equality_behavior() -> EqualityBehavior<Self::Value> {
                EqualityBehavior::Compare(|x, y| match (x, y) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                })
            }
            fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
                OkPagableValueSerialize::<Self::Value>::new()
            }
        }

        self.compute(&ImportPathsKey { cell_name })
            .await?
            .as_ref()
            .duped_err()
    }
}
