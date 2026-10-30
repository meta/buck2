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

/// How the contents of a loaded file are parsed. Every loadable file has one of
/// these, whether it came from the extension or from an `?as=` hint in a
/// `load()` string (e.g. `load(":uv.lock?as=toml", "value")`).
#[derive(
    Clone, Copy, Hash, StrongHash, Eq, PartialEq, Ord, PartialOrd, Debug, Allocative, Pagable
)]
pub enum ImportFormat {
    Bzl,
    Json,
    Toml,
}

impl ImportFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportFormat::Bzl => "bzl",
            ImportFormat::Json => "json",
            ImportFormat::Toml => "toml",
        }
    }

    /// The format a file extension names, if it names one. No "unrecognised
    /// means `Bzl`" fallback: that is the case where something else has to name
    /// a format.
    pub fn from_known_extension(extension: Option<&str>) -> Option<ImportFormat> {
        match extension? {
            "bzl" => Some(ImportFormat::Bzl),
            other => DataFormat::parse(other).map(ImportFormat::from),
        }
    }

    fn as_data(self) -> Option<DataFormat> {
        match self {
            ImportFormat::Bzl => None,
            ImportFormat::Json => Some(DataFormat::Json),
            ImportFormat::Toml => Some(DataFormat::Toml),
        }
    }
}

impl Display for ImportFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The formats that a `?as=` hint can name. Not `bzl`: a file is only evaluated as
/// Starlark if its extension says so.
#[derive(Clone, Copy, Eq, PartialEq, Debug)]
pub enum DataFormat {
    Json,
    Toml,
}

impl DataFormat {
    pub fn as_str(self) -> &'static str {
        ImportFormat::from(self).as_str()
    }

    pub fn parse(name: &str) -> Option<DataFormat> {
        match name {
            "json" => Some(DataFormat::Json),
            "toml" => Some(DataFormat::Toml),
            _ => None,
        }
    }
}

impl From<DataFormat> for ImportFormat {
    fn from(format: DataFormat) -> ImportFormat {
        match format {
            DataFormat::Json => ImportFormat::Json,
            DataFormat::Toml => ImportFormat::Toml,
        }
    }
}

impl Display for DataFormat {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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
    /// How the file is parsed, from an `?as=` hint or from the extension. Part
    /// of the path's identity, so the module type is never re-guessed.
    ///
    /// Not an `Option`: whether a hint was written is derivable (see
    /// [`ImportPath::format_override`]), and storing it too would let
    /// `:a.toml?as=toml` and `:a.toml` be two `EvalImportKey`s for one file.
    format: ImportFormat,
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
        Self::new_for_load_with_format_hint(path, loader_cell, eval_cell, None)
    }

    /// As [`ImportPath::new_for_load`], with the `?as=` hint of the load string, which
    /// must already be stripped from `path`. A hint naming the format that the extension
    /// implies is dropped.
    pub fn new_for_load_with_format_hint(
        path: CellPath,
        loader_cell: BuildFileCell,
        eval_cell: BzlEvalCell,
        format_hint: Option<DataFormat>,
    ) -> buck2_error::Result<Self> {
        let build_file_cell = match eval_cell {
            BzlEvalCell::Loader => loader_cell,
            BzlEvalCell::Own => BuildFileCell::new(path.cell()),
        };
        Self::new_normalized(path, build_file_cell, format_hint.map(ImportFormat::from))
    }

    fn new_with_build_file_cell(
        path: CellPath,
        build_file_cell: BuildFileCell,
    ) -> buck2_error::Result<Self> {
        Self::new_normalized(path, build_file_cell, None)
    }

    /// The one constructor. `build_file_cell` and `format` are normalized here,
    /// so one file loaded one way is one `ImportPath` however it was spelled.
    fn new_normalized(
        path: CellPath,
        build_file_cell: BuildFileCell,
        format: Option<ImportFormat>,
    ) -> buck2_error::Result<Self> {
        if path.parent().is_none() {
            return Err(ImportPathError::Invalid(path).into());
        }

        if path.path().as_str().contains('?') {
            return Err(ImportPathError::Invalid(path).into());
        }

        // A hint wins; failing that the extension is the only thing left.
        let implied = ImportFormat::from_known_extension(path.path().extension());
        let Some(format) = format.or(implied) else {
            return Err(ImportPathError::Suffix(path).into());
        };

        Ok(Self {
            path,
            build_file_cell,
            format,
        })
    }

    /// LSP creates imports for non-bzl files, and puts every one in a
    /// `LoadFile`. Hence `Bzl` outright: the extension is what this ignores.
    pub fn new_hack_for_lsp(path: CellPath) -> buck2_error::Result<Self> {
        let build_file_cell = BuildFileCell::new(path.cell());
        Self::new_normalized(path, build_file_cell, Some(ImportFormat::Bzl))
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

    /// How this file is parsed.
    pub fn format(&self) -> ImportFormat {
        self.format
    }

    /// The format, when the extension does not already name it: what a `load()`
    /// would have had to say after `?as=`. For diagnostics and policy; to parse
    /// the file, use [`ImportPath::format`].
    pub fn format_override(&self) -> Option<DataFormat> {
        let implied = ImportFormat::from_known_extension(self.path.path().extension());
        if implied == Some(self.format) {
            return None;
        }
        self.format.as_data()
    }

    /// Parent directory of the import path.
    pub fn path_parent(&self) -> CellPath {
        self.path
            .parent()
            .expect("constructor verified path has parent")
    }
}

impl Display for ImportPath {
    /// Format and build file cell included, else two modules render alike.
    /// `{:#}` is the file alone, for messages about its contents.
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)?;
        if f.alternate() {
            return Ok(());
        }
        if let Some(format) = self.format_override() {
            write!(f, "?as={format}")?;
        }
        if self.build_file_cell.name() != self.path.cell() {
            write!(f, "@{}", self.build_file_cell.name())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell_path(cell: &str, filename: &str) -> CellPath {
        CellPath::new(
            CellName::testing_new(cell),
            &CellRelativePath::unchecked_new("pkg").join(FileName::unchecked_new(filename)),
        )
    }

    /// A plain `load()` with no `?as=` hint, as resolved for a build file in the
    /// same cell.
    fn import(filename: &str) -> buck2_error::Result<ImportPath> {
        ImportPath::new_for_load(
            cell_path("root", filename),
            BuildFileCell::new(CellName::testing_new("root")),
            BzlEvalCell::Loader,
        )
    }

    /// A `load()` carrying an `?as=` hint.
    fn import_as(filename: &str, format: DataFormat) -> buck2_error::Result<ImportPath> {
        ImportPath::new_for_load_with_format_hint(
            cell_path("root", filename),
            BuildFileCell::new(CellName::testing_new("root")),
            BzlEvalCell::Loader,
            Some(format),
        )
    }

    #[test]
    fn no_hint_means_the_format_is_inferred_from_the_extension() {
        for (filename, format) in [
            ("a.bzl", ImportFormat::Bzl),
            ("a.json", ImportFormat::Json),
            ("a.toml", ImportFormat::Toml),
        ] {
            let import = import(filename).unwrap();
            assert_eq!(format, import.format(), "{filename}");
            assert_eq!(None, import.format_override(), "{filename}");
        }
    }

    #[test]
    fn unknown_extension_requires_an_explicit_format() {
        for filename in ["Cargo.lock", "Makefile", "rules.bzl.in"] {
            assert!(import(filename).is_err(), "{filename}");

            let explicit = import_as(filename, DataFormat::Toml).unwrap();
            assert_eq!(ImportFormat::Toml, explicit.format(), "{filename}");
            assert_eq!(
                Some(DataFormat::Toml),
                explicit.format_override(),
                "{filename}"
            );
        }
    }

    #[test]
    fn an_lsp_path_never_renders_an_as_bzl_hint() {
        for filename in ["BUCK", "a.json"] {
            let lsp = ImportPath::new_hack_for_lsp(cell_path("root", filename)).unwrap();
            assert_eq!(ImportFormat::Bzl, lsp.format(), "{filename}");
            assert_eq!(None, lsp.format_override(), "{filename}");
            assert_eq!(format!("root//pkg/{filename}"), lsp.to_string());
        }
    }

    #[test]
    fn explicit_format_overrides_the_extension() {
        let as_toml = import_as("a.json", DataFormat::Toml).unwrap();
        assert_eq!(ImportFormat::Toml, as_toml.format());
        // The same file under two formats must stay two distinct modules.
        assert_ne!(import("a.json").unwrap(), as_toml);
    }

    #[test]
    fn redundant_format_collapses_so_the_module_is_not_duplicated() {
        // A hint restating the extension must leave no trace, so `:a.toml?as=toml`
        // and `:a.toml` are one module. Two `ImportPath`s are two `EvalImportKey`s,
        // i.e. the file read and parsed twice, handing out two copies of itself.
        for (filename, format) in [("a.json", DataFormat::Json), ("a.toml", DataFormat::Toml)] {
            let explicit = import_as(filename, format).unwrap();
            assert_eq!(import(filename).unwrap(), explicit, "{filename}");
            assert_eq!(None, explicit.format_override(), "{filename}");
            assert_eq!(ImportFormat::from(format), explicit.format(), "{filename}");
        }
    }

    #[test]
    fn display_round_trips_an_explicit_format() {
        assert_eq!("root//pkg/a.bzl", import("a.bzl").unwrap().to_string());
        assert_eq!(
            "root//pkg/Cargo.lock?as=toml",
            import_as("Cargo.lock", DataFormat::Toml)
                .unwrap()
                .to_string()
        );
    }

    #[test]
    fn the_alternate_display_is_the_file_alone() {
        let hinted = import_as("Cargo.lock", DataFormat::Toml).unwrap();
        assert_eq!("root//pkg/Cargo.lock", format!("{hinted:#}"));

        let cross_cell = ImportPath::new_for_load_with_format_hint(
            cell_path("root", "Cargo.lock"),
            BuildFileCell::new(CellName::testing_new("other")),
            BzlEvalCell::Loader,
            Some(DataFormat::Toml),
        )
        .unwrap();
        assert_eq!("root//pkg/Cargo.lock", format!("{cross_cell:#}"));
        assert_eq!(
            "root//pkg/Cargo.lock?as=toml@other",
            format!("{cross_cell}")
        );
    }

    #[test]
    fn a_path_containing_a_question_mark_is_still_rejected() {
        assert!(import_as("a.bzl?as=toml", DataFormat::Toml).is_err());
    }
}
