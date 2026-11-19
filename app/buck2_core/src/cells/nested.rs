/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use pagable::Pagable;

use crate::cells::cell_root_path::CellRootPath;
use crate::cells::name::CellName;
use crate::cells::paths::CellRelativePath;
use crate::cells::paths::CellRelativePathBuf;

/// Paths to cells which reside inside current cell.
///
/// Target labels cannot cross cell boundaries. This utility helps to identify such targets.
#[derive(Eq, PartialEq, Debug, Allocative, Clone, Pagable)]
pub struct NestedCells {
    paths: Vec<(CellRelativePathBuf, CellName)>,
}

impl NestedCells {
    fn from_cell_paths_relative_to_this_cell<'a>(
        paths: impl IntoIterator<Item = (&'a CellRelativePath, CellName)>,
    ) -> NestedCells {
        let mut paths: Vec<_> = paths.into_iter().filter(|(p, _)| !p.is_empty()).collect();

        let mut final_paths: Vec<(CellRelativePathBuf, CellName)> = Vec::new();

        paths.sort();

        // Remove redundant paths, e.g. if `paths` contains `a` and `a/b`, then `a/b` is redundant.
        for (path, cell_name) in paths {
            if let Some((last_path, _)) = final_paths.last() {
                if path.starts_with(last_path) {
                    continue;
                }
            }

            final_paths.push((path.to_owned(), cell_name));
        }

        NestedCells { paths: final_paths }
    }

    pub fn from_cell_roots(
        all_cells: &[(CellName, &CellRootPath)],
        this_cell: &CellRootPath,
    ) -> NestedCells {
        Self::from_cell_paths_relative_to_this_cell(all_cells.iter().filter_map(
            |(cell_name, cell_root_path)| {
                let path_relative_to_this_cell = cell_root_path.strip_prefix_opt(this_cell)?;

                Some((
                    CellRelativePath::new(path_relative_to_this_cell),
                    *cell_name,
                ))
            },
        ))
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The cells rooted directly under `dir`, by name. Directory listings leave nested cell
    /// roots out, so a caller walking a directory uses this to descend into them.
    pub fn cells_rooted_under<'a>(
        &'a self,
        dir: &'a CellRelativePath,
    ) -> impl Iterator<Item = CellName> + 'a {
        self.paths
            .iter()
            .filter_map(move |(root, name)| (root.parent() == Some(dir)).then_some(*name))
    }

    pub fn matches<'a, 'b>(
        &'a self,
        path: &'b CellRelativePath,
    ) -> Option<(&'a CellRelativePath, CellName, &'b ForwardRelativePath)> {
        for (cell_path, cell_name) in &self.paths {
            if let Some(rem) = path.strip_prefix_opt(cell_path) {
                return Some((cell_path, *cell_name, rem));
            }
        }
        None
    }

    pub(crate) fn check_empty(&self) -> Option<CellName> {
        self.paths.first().map(|(_, cell_name)| *cell_name)
    }
}

#[cfg(test)]
mod tests {

    use crate::cells::name::CellName;
    use crate::cells::nested::NestedCells;
    use crate::cells::paths::CellRelativePath;
    use crate::cells::paths::CellRelativePathBuf;

    #[test]
    fn test_nested_cells() {
        let nested_cells = NestedCells::from_cell_paths_relative_to_this_cell([
            (
                CellRelativePath::testing_new("cell_a/cell_b/cell_c"),
                CellName::testing_new("abc"),
            ),
            (
                CellRelativePath::testing_new("cell_a"),
                CellName::testing_new("a"),
            ),
            (
                CellRelativePath::testing_new("cell_x/cell_y"),
                CellName::testing_new("xy"),
            ),
            (
                CellRelativePath::testing_new("cell_a/cell_b"),
                CellName::testing_new("ab"),
            ),
        ]);

        assert_eq!(
            NestedCells {
                paths: vec![
                    (
                        CellRelativePathBuf::testing_new("cell_a"),
                        CellName::testing_new("a")
                    ),
                    (
                        CellRelativePathBuf::testing_new("cell_x/cell_y"),
                        CellName::testing_new("xy")
                    )
                ]
            },
            nested_cells,
        );
    }
}
