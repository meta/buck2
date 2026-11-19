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
use buck2_common::file_ops::dice::DiceFileComputations;
use buck2_common::file_ops::error::FileReadErrorContext;
use buck2_common::file_ops::metadata::RawPathMetadata;
use buck2_common::legacy_configs::dice::HasLegacyConfigs;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::view::LegacyBuckConfigView;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::cell_path_with_allowed_relative_dir::CellPathWithAllowedRelativeDir;
use buck2_core::pattern::parse_package::parse_cell_path;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::ResultDupedErrExt;
use pagable::Pagable;
use pagable::pagable_typetag;

#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum RelativePathParseError {
    #[error("Multiple relative-enabled directory prefixes found for the current dir: `{0}`.")]
    MultipleAllowedDirsForCurrentDir(String),
    #[error(
        "Symlink found on the way from current dir `{0}` to allowed relative dir `{1}`: `{2}`."
    )]
    SymlinkFound(String, String, String),
}

#[async_trait]
pub trait HasAllowRelativePaths<'d> {
    async fn dirs_allowing_relative_paths(
        &mut self,
        cell_path: CellPath,
    ) -> buck2_error::Result<&'d Arc<CellPathWithAllowedRelativeDir>>;
}

#[async_trait]
impl<'d> HasAllowRelativePaths<'d> for DiceComputations<'d> {
    async fn dirs_allowing_relative_paths(
        &mut self,
        cell_path: CellPath,
    ) -> buck2_error::Result<&'d Arc<CellPathWithAllowedRelativeDir>> {
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
        #[display("{}", cell_path)]
        #[pagable_typetag(dice::DiceKeyDyn)]
        struct AllowRelativePathsKey {
            cell_path: CellPath,
        }

        #[async_trait]
        impl Key for AllowRelativePathsKey {
            type Value = buck2_error::Result<Arc<CellPathWithAllowedRelativeDir>>;

            async fn compute(
                &self,
                ctx: &mut DiceComputations,
                _cancellation: &CancellationContext,
            ) -> Self::Value {
                let config = ctx.get_legacy_root_config_on_dice().await?;
                let cell_resolver = ctx.get_cell_resolver().await?;
                let cell_alias_resolver = cell_resolver.root_cell_cell_alias_resolver();

                let allowed_relative_dirs_for_current_dir = config
                    .view(ctx)
                    .parse_list::<String>(BuckconfigKeyRef {
                        section: "buck2",
                        property: "directories_to_allow_relative_paths",
                    })?
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|dir| {
                        parse_cell_path(dir.trim(), cell_alias_resolver)
                            .map(|allow_dir| {
                                if self.cell_path.starts_with(allow_dir) {
                                    Some(allow_dir)
                                } else {
                                    None
                                }
                            })
                            .transpose()
                    })
                    .collect::<Result<Vec<CellPath>, _>>()?;

                if allowed_relative_dirs_for_current_dir.len() > 1 {
                    return Err(RelativePathParseError::MultipleAllowedDirsForCurrentDir(
                        self.cell_path.to_string(),
                    )
                    .into());
                }

                let allowed_relative_dir = allowed_relative_dirs_for_current_dir.first();

                if let Some(allowed_relative_dir) = allowed_relative_dir {
                    validate_no_symlinks_between_current_dir_and_allowed_dir(
                        ctx,
                        self.cell_path,
                        *allowed_relative_dir,
                    )
                    .await?;
                }

                Ok(Arc::new(CellPathWithAllowedRelativeDir::new(
                    self.cell_path,
                    allowed_relative_dir.cloned(),
                )))
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

        self.compute(&AllowRelativePathsKey { cell_path })
            .await?
            .as_ref()
            .duped_err()
    }
}

async fn validate_no_symlinks_between_current_dir_and_allowed_dir(
    ctx: &mut DiceComputations<'_>,
    current_dir: CellPath,
    allowed_dir: CellPath,
) -> buck2_error::Result<()> {
    // For all the paths between current_dir and allowed_dir, inclusive
    // check that there are no symlinks
    let mut current_dir = current_dir;
    loop {
        if let RawPathMetadata::Symlink { at, to: _ } =
            DiceFileComputations::read_path_metadata(ctx, current_dir)
                .await
                .without_package_context_information()?
        {
            return Err(RelativePathParseError::SymlinkFound(
                current_dir.to_string(),
                allowed_dir.to_string(),
                at.to_string(),
            )
            .into());
        }
        if current_dir == allowed_dir {
            break;
        }
        current_dir = current_dir
            .parent()
            .expect("current dir should have parent if not equal to allowed dir");
    }
    Ok(())
}
