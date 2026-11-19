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
use buck2_core::cells::CellResolver;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use pagable::Pagable;

use crate::ignores::ignore_set::IgnoreSet;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
enum FileOpsError {
    #[error("Tried to read ignored dir `{0}` (reason: {1}).")]
    ReadIgnoredDir(String, String),
}

#[derive(Debug, Allocative)]
pub enum FileIgnoreReason {
    IgnoredByPattern { path: String, pattern: String },
    IgnoredByCell { path: String, cell_name: CellName },
}

impl FileIgnoreReason {
    pub fn describe(&self) -> String {
        match self {
            FileIgnoreReason::IgnoredByPattern { pattern, .. } => {
                format!("config project.ignore contains `{pattern}`")
            }
            FileIgnoreReason::IgnoredByCell { cell_name, .. } => {
                format!("path is contained in cell `{cell_name}`")
            }
        }
    }
}

#[derive(Debug, Allocative)]
pub enum FileIgnoreResult {
    Ok,
    Ignored(FileIgnoreReason),
}

impl FileIgnoreReason {
    /// The error for having tried to read the ignored path.
    pub fn into_read_error(self) -> buck2_error::Error {
        match self {
            FileIgnoreReason::IgnoredByPattern { path, pattern } => FileOpsError::ReadIgnoredDir(
                path,
                format!("file is matched by pattern `{pattern}`"),
            )
            .into(),
            FileIgnoreReason::IgnoredByCell { path, cell_name } => {
                FileOpsError::ReadIgnoredDir(path, format!("file is part of cell `{cell_name}`"))
                    .into()
            }
        }
    }
}

impl FileIgnoreResult {
    /// Converts the FileIgnoreResult to a Result<()> where any ignored case is converted to an Err
    /// with appropriate message. This should be used when it would be an error to interact with an
    /// ignored file.
    pub fn into_result(self) -> buck2_error::Result<()> {
        match self {
            FileIgnoreResult::Ok => Ok(()),
            FileIgnoreResult::Ignored(reason) => Err(reason.into_read_error()),
        }
    }

    /// Returns true if the file is ignored, false otherwise.
    pub fn is_ignored(&self) -> bool {
        !matches!(self, FileIgnoreResult::Ok)
    }
}

/// Ignores files based on configured ignore patterns and nested cell roots,
/// viewed from a particular cell.
#[derive(PartialEq, Eq, Allocative, Debug, Pagable)]
pub struct CellFileIgnores {
    cells: CellResolver,
    cell: CellName,
    ignores: Arc<IgnoreSet>,
}

impl CellFileIgnores {
    pub fn new(cells: CellResolver, cell: CellName, ignores: Arc<IgnoreSet>) -> Self {
        // Verify the cell is present in the resolver up front so that `check`
        // does not have to handle a fallible lookup.
        assert!(
            cells.get(cell).is_ok(),
            "cell `{cell}` missing from CellResolver"
        );
        CellFileIgnores {
            cells,
            cell,
            ignores,
        }
    }

    pub(crate) fn check(&self, path: &CellRelativePath) -> FileIgnoreResult {
        let instance = self
            .cells
            .get(self.cell)
            .expect("cell exists in resolver, validated in new()");

        let project_path = instance.path().join(path);
        if let Some(pattern) = self.ignores.matches_candidate(&project_path) {
            return FileIgnoreResult::Ignored(FileIgnoreReason::IgnoredByPattern {
                path: path.as_str().to_owned(),
                pattern: pattern.to_owned(),
            });
        }

        if let Some((_, cell_name, _)) = instance.nested_cells().matches(path) {
            return FileIgnoreResult::Ignored(FileIgnoreReason::IgnoredByCell {
                path: path.as_str().to_owned(),
                cell_name,
            });
        }

        FileIgnoreResult::Ok
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use buck2_core::cells::CellResolver;
    use buck2_core::cells::cell_root_path::CellRootPathBuf;
    use buck2_core::cells::name::CellName;
    use buck2_core::cells::paths::CellRelativePath;
    use dupe::Dupe;

    use crate::ignores::file_ignores::CellFileIgnores;
    use crate::ignores::ignore_set::IgnoreSet;

    #[test]
    fn file_ignores() -> buck2_error::Result<()> {
        // The test cell is intentionally not at the project root. A separate
        // cell at the actual project root is included so the CellResolver
        // accepts the setup.
        let test_cell = CellName::testing_new("test_root");
        let cells = CellResolver::testing_with_names_and_paths(&[
            (
                CellName::testing_new("root"),
                CellRootPathBuf::testing_new(""),
            ),
            (test_cell, CellRootPathBuf::testing_new("root")),
            (
                CellName::testing_new("other"),
                CellRootPathBuf::testing_new("root/other_cell"),
            ),
            (
                CellName::testing_new("third"),
                CellRootPathBuf::testing_new("third"),
            ),
        ]);

        let ignore_set = Arc::new(IgnoreSet::testing_new_single_cell(
            cells.dupe(),
            test_cell,
            "**/*.java , some/dir/**, one/*, \n    recursive, trailing_slash/",
        )?);
        let ignores = CellFileIgnores::new(cells, test_cell, ignore_set);

        assert!(
            ignores
                .check(CellRelativePath::unchecked_new("some/long/path/Class.java"))
                .is_ignored()
        );

        assert!(
            ignores
                .check(CellRelativePath::unchecked_new("other_cell"))
                .is_ignored()
        );

        assert!(
            ignores
                .check(CellRelativePath::unchecked_new("other_cell/some/lib"))
                .is_ignored()
        );

        assert!(
            !ignores
                .check(CellRelativePath::unchecked_new("third"))
                .is_ignored()
        );

        assert!(
            !ignores
                .check(CellRelativePath::unchecked_new("one/two/three"))
                .is_ignored()
        );

        assert!(
            ignores
                .check(CellRelativePath::unchecked_new("recursive/two/three"))
                .is_ignored()
        );

        assert!(
            ignores
                .check(CellRelativePath::unchecked_new("trailing_slash/BUCK"))
                .is_ignored()
        );

        Ok(())
    }
}
