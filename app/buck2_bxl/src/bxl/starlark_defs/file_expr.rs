/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::path::Path;

use buck2_build_api::interpreter::rule_defs::artifact::starlark_artifact_like::StarlarkArtifactView;
use buck2_common::dice::cells::HasCellResolver;
use buck2_common::dice::data::HasProjectRoot;
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::instance::CellInstance;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::pattern::parse_package::maybe_parse_cell_path;
use buck2_fs::paths::abs_path::AbsPath;
use derive_more::Display;
use dice::DiceComputations;
use dupe::Dupe;
use starlark::values::UnpackValue;
use starlark::values::ValueOf;

use crate::bxl::starlark_defs::file_set::StarlarkFileNode;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum FileExprError {
    #[error("Expected a source artifact, got `{0}`")]
    NotASourceArtifact(String),
}

/// FileExpr is just a simple type that can be used in starlark_module
/// functions for arguments that should be files. It will accept either a
/// literal (like `//path/to/some/file.txt`) or a `SourceArtifact` via `StarlarkArtifact`
/// or a `StarlarkFileNode`
#[derive(Debug, Display, Clone, UnpackValue)]
pub(crate) enum FileExpr<'v> {
    Literal(&'v str),
    SourceArtifact(StarlarkArtifactView<'v>),
    StarlarkFileNode(ValueOf<'v, StarlarkFileNode>),
}

fn parse_cell_path_as_file_expr_literal(
    val: &str,
    cell_alias_resolver: &CellAliasResolver,
) -> buck2_error::Result<Option<CellPath>> {
    maybe_parse_cell_path(val, cell_alias_resolver)
}

impl<'a> FileExpr<'a> {
    pub(crate) async fn get(
        self,
        dice: &mut DiceComputations<'_>,
        cell_instance: &CellInstance,
    ) -> buck2_error::Result<CellPath> {
        match self {
            FileExpr::Literal(val) => {
                let cell_alias_resolver =
                    dice.get_cell_alias_resolver(cell_instance.name()).await?;
                match parse_cell_path_as_file_expr_literal(val, &cell_alias_resolver)? {
                    Some(cell_path) => Ok(cell_path),
                    None => {
                        let fs = dice.get_project_root().await?.dupe();
                        let path = Path::new(val);
                        let rel = if path.is_absolute() {
                            Cow::Owned(fs.relativize_any(AbsPath::new(path)?)?)
                        } else {
                            Cow::Borrowed(<&ProjectRelativePath>::try_from(val)?)
                        };
                        Ok(dice.get_cell_resolver().await?.get_cell_path(&rel))
                    }
                }
            }
            FileExpr::SourceArtifact(artifact) => {
                let bound = artifact.as_input_artifact_like().get_bound_artifact()?;
                match bound.get_source() {
                    Some(source) => Ok(source.get_path().to_cell_path()),
                    None => Err(FileExprError::NotASourceArtifact(artifact.to_string()).into()),
                }
            }
            FileExpr::StarlarkFileNode(val) => Ok(val.0),
        }
    }
}

#[cfg(test)]
mod tests {

    use buck2_core::cells::alias::NonEmptyCellAlias;
    use buck2_core::cells::name::CellName;
    use buck2_hash::IntentionallyStdHashMap;

    use super::*;

    #[test]
    fn test_parse_cell_path_as_file_expr_literal() -> buck2_error::Result<()> {
        let cell1 = CellName::testing_new("cell1");

        let map = IntentionallyStdHashMap::from([
            (
                NonEmptyCellAlias::new("cell1".to_owned()).unwrap(),
                CellName::testing_new("cell1"),
            ),
            (
                NonEmptyCellAlias::new("cell2".to_owned()).unwrap(),
                CellName::testing_new("cell2"),
            ),
            (
                NonEmptyCellAlias::new("cell3".to_owned()).unwrap(),
                CellName::testing_new("cell3"),
            ),
        ]);

        let cell_alias_resolver = CellAliasResolver::new(cell1, map)?;

        let actual_cell_path =
            parse_cell_path_as_file_expr_literal("//foo/bar", &cell_alias_resolver)?.unwrap();
        let expected_cell_path = CellPath::testing_new("cell1//foo/bar");
        assert_eq!(actual_cell_path, expected_cell_path);

        let actual_cell_path =
            parse_cell_path_as_file_expr_literal("@//foo/bar", &cell_alias_resolver)?.unwrap();
        let expected_cell_path = CellPath::testing_new("cell1//foo/bar");
        assert_eq!(actual_cell_path, expected_cell_path);

        let actual_cell_path =
            parse_cell_path_as_file_expr_literal("@cell1//foo/bar", &cell_alias_resolver)?.unwrap();
        let expected_cell_path = CellPath::testing_new("cell1//foo/bar");
        assert_eq!(actual_cell_path, expected_cell_path);

        let actual_cell_path =
            parse_cell_path_as_file_expr_literal("cell2//foo/bar", &cell_alias_resolver)?.unwrap();
        let expected_cell_path = CellPath::testing_new("cell2//foo/bar");
        assert_eq!(actual_cell_path, expected_cell_path);

        let actual_cell_path =
            parse_cell_path_as_file_expr_literal("@cell3//foo/bar", &cell_alias_resolver)?.unwrap();
        let expected_cell_path = CellPath::testing_new("cell3//foo/bar");
        assert_eq!(actual_cell_path, expected_cell_path);

        assert!(
            parse_cell_path_as_file_expr_literal("/abs/path/foo/bar", &cell_alias_resolver)?
                .is_none()
        );
        assert!(
            parse_cell_path_as_file_expr_literal("rel/path/foo/bar", &cell_alias_resolver)?
                .is_none()
        );

        Ok(())
    }
}
