/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use buck2_fs::paths::file_name::FileName;
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::cells::build_file_cell::BuildFileCell;
use crate::cells::cell_path::CellPath;
use crate::cells::name::CellName;
use crate::cells::paths::CellRelativePath;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum ImportPathError {
    #[error("Invalid import path `{0}`")]
    Invalid(CellPath),
    #[error("Import path must have suffix `.bzl`, `.json`, or `.toml`: `{0}`")]
    Suffix(CellPath),
}

/// The cell that the top level of a loaded file is evaluated for: the cell whose buckconfig
/// `read_config()` reads, that `get_cell_name()` returns, and that a label without a cell
/// (`//foo:bar`) is in.
///
/// Functions that the file defines are not affected. They see the cell of whichever file is
/// being evaluated when they are called.
#[derive(Clone, Copy, Dupe, Debug, Eq, PartialEq, Allocative, Pagable)]
pub enum BzlEvalCell {
    /// The cell of the build file, `PACKAGE` file or `bxl` file that loads the file, perhaps
    /// transitively. A file that is loaded from several cells is evaluated once for each.
    Loader,
    /// The cell that the file is in. Every file is evaluated once.
    Own,
}

/// Path of a `.bzl` file.
#[derive(
    Clone, Hash, StrongHash, Eq, PartialEq, Ord, PartialOrd, Debug, Allocative, Pagable
)]
pub struct ImportPath {
    /// The path to the import as a 'CellPath', which contains the cell
    /// information and the cell relative path to the bzl file itself, including the bzl suffix
    path: CellPath,
    /// The cell that the file is evaluated for, see `BzlEvalCell`.
    build_file_cell: BuildFileCell,
}

impl ImportPath {
    /// The path of a file that is evaluated for its own cell whatever `BzlEvalCell` says: a
    /// file of the prelude, or one that a command names rather than a `load()`.
    pub fn new_same_cell(path: CellPath) -> buck2_error::Result<Self> {
        let build_file_cell = BuildFileCell::new(path.cell());
        Self::new_with_build_file_cell(path, build_file_cell)
    }

    /// The path of a file that is loaded by a file which is itself evaluated for
    /// `loader_cell`.
    pub fn new_for_load(
        path: CellPath,
        loader_cell: BuildFileCell,
        eval_cell: BzlEvalCell,
    ) -> buck2_error::Result<Self> {
        match eval_cell {
            BzlEvalCell::Loader => Self::new_with_build_file_cell(path, loader_cell),
            BzlEvalCell::Own => Self::new_same_cell(path),
        }
    }

    fn new_with_build_file_cell(
        path: CellPath,
        build_file_cell: BuildFileCell,
    ) -> buck2_error::Result<Self> {
        if path.parent().is_none() {
            return Err(ImportPathError::Invalid(path).into());
        }

        if path.path().as_str().contains('?') {
            return Err(ImportPathError::Invalid(path).into());
        }

        if !matches!(path.path().extension(), Some("bzl" | "json" | "toml")) {
            return Err(ImportPathError::Suffix(path).into());
        }

        Ok(Self {
            path,
            build_file_cell,
        })
    }

    /// LSP creates imports for non-bzl files.
    pub fn new_hack_for_lsp(path: CellPath) -> buck2_error::Result<Self> {
        if path.parent().is_none() {
            return Err(ImportPathError::Invalid(path).into());
        }

        if path.path().as_str().contains('?') {
            return Err(ImportPathError::Invalid(path).into());
        }

        Ok(Self {
            path,
            build_file_cell: BuildFileCell::new(path.cell()),
        })
    }

    pub fn testing_new(path: &str) -> Self {
        let (cell, rem) = path.split_once("//").unwrap();
        let (cell_relative_path, filename) = rem.rsplit_once(':').unwrap();
        Self::testing_new_cross_cell(cell, cell_relative_path, filename, cell)
    }

    pub fn testing_new_cross_cell(
        cell: &str,
        cell_relative_path: &str,
        filename: &str,
        build_file_cell: &str,
    ) -> Self {
        let path = CellRelativePath::unchecked_new(cell_relative_path)
            .join(FileName::unchecked_new(filename));
        let cell_path = CellPath::new(CellName::testing_new(cell), &path);
        Self::new_with_build_file_cell(
            cell_path,
            BuildFileCell::new(CellName::testing_new(build_file_cell)),
        )
        .unwrap()
    }

    pub fn cell(&self) -> CellName {
        self.path.cell()
    }

    pub fn build_file_cell(&self) -> BuildFileCell {
        self.build_file_cell
    }

    pub fn path(&self) -> &CellPath {
        &self.path
    }

    /// Parent directory of the import path.
    pub fn path_parent(&self) -> CellPath {
        self.path
            .parent()
            .expect("constructor verified path has parent")
    }
}

impl Display for ImportPath {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.build_file_cell.name() == self.path.cell() {
            write!(f, "{}", self.path)
        } else {
            write!(f, "{}@{}", self.path, self.build_file_cell.name())
        }
    }
}
