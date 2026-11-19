/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::alias::NonEmptyCellAlias;
use buck2_core::cells::cell_root_path::CellRootPath;
use buck2_core::cells::cell_root_path::CellRootPathBuf;
use buck2_core::cells::external::ExternalCellOrigin;
use buck2_core::cells::instance;
use buck2_core::cells::name::CellName;
use buck2_core::cells::nested::NestedCells;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckMutMap;
use buck2_hash::IntentionallyStdHashMap;
use instance::CellInstance;

/// Errors from cell creation
#[derive(buck2_error::Error, Debug)]
#[buck2(input)]
enum CellError {
    #[error(
        "Cell name `{0}` should be an alias for an existing cell, but `{1}` isn't a known alias"
    )]
    AliasOnlyCell(NonEmptyCellAlias, NonEmptyCellAlias),
    #[error("No cell name for the root path, add an entry for `.`")]
    NoRootCell,
    #[error("`{0}` is not a known cell alias")]
    UnknownCellAlias(NonEmptyCellAlias),
    #[error("`{0}` was provided both as a cell name and as an alias")]
    AliasAndName(NonEmptyCellAlias),
    #[error("Cell `{0}` was marked as external twice")]
    DuplicateExternalCell(CellName),
}

/// Aggregates cell information as we parse cell configs and keeps state to
/// generate a final 'CellResolver'
#[derive(Debug)]
pub(crate) struct CellsAggregator {
    cell_infos: BuckMutMap<CellName, CellAggregatorInfo>,
    root_aliases: IntentionallyStdHashMap<NonEmptyCellAlias, CellName>,
    root_cell: CellName,
}

#[derive(Debug)]
struct CellAggregatorInfo {
    path: CellRootPathBuf,
    external: Option<ExternalCellOrigin>,
}

impl CellsAggregator {
    pub(crate) fn new(
        // This is order sensitive
        cells: Vec<(CellName, CellRootPathBuf)>,
        root_aliases: BuckMutMap<NonEmptyCellAlias, NonEmptyCellAlias>,
    ) -> buck2_error::Result<Self> {
        let mut path_rmap = BuckMutMap::default();
        let mut infos = BuckMutMap::default();
        let mut combined_aliases = IntentionallyStdHashMap::new();
        for (cell, path) in cells {
            let real_cell = match path_rmap.try_insert(path.clone(), cell) {
                Ok(_) => {
                    infos.insert(
                        cell,
                        CellAggregatorInfo {
                            path,
                            external: None,
                        },
                    );
                    cell
                }
                Err(occupied) => *occupied.entry.get(),
            };
            combined_aliases.insert(NonEmptyCellAlias::new(cell.as_str().to_owned())?, real_cell);
        }

        let Some(&root_cell) = path_rmap.get(CellRootPath::new(ProjectRelativePath::empty()))
        else {
            return Err(CellError::NoRootCell.into());
        };

        for (from, to) in root_aliases {
            let Some(cell) = combined_aliases.get(&to) else {
                return Err(CellError::AliasOnlyCell(from, to).into());
            };
            if combined_aliases.insert(from.clone(), *cell).is_some() {
                return Err(CellError::AliasAndName(from).into());
            }
        }

        Ok(Self {
            cell_infos: infos,
            root_aliases: combined_aliases,
            root_cell,
        })
    }

    pub(crate) fn resolve_root_alias(
        &self,
        alias: NonEmptyCellAlias,
    ) -> buck2_error::Result<CellName> {
        self.root_aliases
            .get(&alias)
            .copied()
            .ok_or_else(|| CellError::UnknownCellAlias(alias).into())
    }

    pub(crate) fn mark_external_cell(
        &mut self,
        cell: CellName,
        origin: ExternalCellOrigin,
    ) -> buck2_error::Result<()> {
        let info = self
            .cell_infos
            .get_mut(&cell)
            .internal_error("cell name is not a cell")?;
        if info.external.is_some() {
            return Err(CellError::DuplicateExternalCell(cell).into());
        }
        info.external = Some(origin);
        Ok(())
    }

    pub(crate) fn make_cell_resolver(self) -> buck2_error::Result<CellResolver> {
        let all_cell_roots_for_nested_cells: Vec<_> = self
            .cell_infos
            .iter()
            .map(|(name, info)| (*name, info.path.as_path()))
            .collect();

        let instances = self
            .cell_infos
            .iter()
            .map(|(name, info)| {
                let nested_cells =
                    NestedCells::from_cell_roots(&all_cell_roots_for_nested_cells, &info.path);
                CellInstance::new(
                    *name,
                    info.path.clone(),
                    info.external.clone(),
                    nested_cells,
                )
            })
            .collect::<buck2_error::Result<Vec<_>>>()?;

        let root_cell_alias_resolver = CellAliasResolver::new(self.root_cell, self.root_aliases)?;

        CellResolver::new(instances, root_cell_alias_resolver)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duplicate_paths() -> buck2_error::Result<()> {
        let root = CellName::testing_new("root");
        let root_path = CellRootPathBuf::new(ProjectRelativePath::empty().to_owned());
        let other1 = CellName::testing_new("other1");
        let other2 = CellName::testing_new("other2");
        let other_path = CellRootPathBuf::new(ProjectRelativePath::new("random/path")?.to_owned());

        let cell_resolver = CellsAggregator::new(
            vec![
                (root, root_path.clone()),
                (other1, other_path.clone()),
                (other2, other_path.clone()),
            ],
            BuckMutMap::default(),
        )
        .unwrap()
        .make_cell_resolver()
        .unwrap();
        assert!(
            cell_resolver
                .get(CellName::testing_new("root"))
                .unwrap()
                .path()
                == root_path.as_path()
        );
        assert!(
            cell_resolver
                .get(CellName::testing_new("other1"))
                .unwrap()
                .path()
                == other_path.as_path()
        );
        Ok(())
    }

    #[test]
    fn test_alias_only_error() {
        assert!(
            CellsAggregator::new(
                Vec::new(),
                BuckMutMap::from_iter([(
                    NonEmptyCellAlias::testing_new("root"),
                    NonEmptyCellAlias::testing_new("does_not_exist")
                )])
            )
            .is_err()
        );
    }
}
