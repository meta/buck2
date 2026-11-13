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

    /// Parse a format name. Whether it may be *requested* via `?as=` is decided
    /// where load strings are parsed; `?as=bzl` is not accepted.
    pub fn parse(name: &str) -> Option<ImportFormat> {
        match name {
            "bzl" => Some(ImportFormat::Bzl),
            "json" => Some(ImportFormat::Json),
            "toml" => Some(ImportFormat::Toml),
            _ => None,
        }
    }

    /// The format a file extension names, if it names one. No "unrecognised
    /// means `Bzl`" fallback: that is the case where something else has to name
    /// a format.
    pub fn from_known_extension(extension: Option<&str>) -> Option<ImportFormat> {
        match extension? {
            "bzl" => Some(ImportFormat::Bzl),
            "json" => Some(ImportFormat::Json),
            "toml" => Some(ImportFormat::Toml),
            _ => None,
        }
    }
}

impl Display for ImportFormat {
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
    /// The cell of the top-level build module that this is being loaded
    /// (perhaps transitively) into.
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
    /// We evaluate `bzl` files multiple times: for each cell we evaluate `bzl` file again.
    /// We want to stop doing that.
    /// This function is for call sites where we don't care about the build file cell.
    pub fn new_same_cell(path: CellPath) -> buck2_error::Result<Self> {
        let build_file_cell = BuildFileCell::new(path.cell());
        Self::new_with_build_file_cells(path, build_file_cell)
    }

    pub fn new_with_build_file_cells(
        path: CellPath,
        build_file_cell: BuildFileCell,
    ) -> buck2_error::Result<Self> {
        Self::new_normalized(path, build_file_cell, None)
    }

    /// Create an ImportPath for a file with an explicit format override (e.g. `?as=toml`).
    /// Skips the file extension validation since the format is determined by the hint,
    /// not the extension.
    ///
    /// Do not include the ?as=toml hint as part of the path.
    pub fn new_with_explicit_format(
        path: CellPath,
        build_file_cell: BuildFileCell,
        format: ImportFormat,
    ) -> buck2_error::Result<Self> {
        Self::new_normalized(path, build_file_cell, Some(format))
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
    pub fn new_hack_for_lsp(
        path: CellPath,
        build_file_cell: BuildFileCell,
    ) -> buck2_error::Result<Self> {
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
        Self::new_with_build_file_cells(
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
    /// would have had to say after `?as=`. Never `Bzl`, since `?as=bzl` cannot
    /// be written; a `Bzl` path the extension doesn't name only comes from
    /// [`ImportPath::new_hack_for_lsp`]. For diagnostics and policy; to parse
    /// the file, use [`ImportPath::format`].
    pub fn format_override(&self) -> Option<ImportFormat> {
        if self.format == ImportFormat::Bzl {
            return None;
        }
        let implied = ImportFormat::from_known_extension(self.path.path().extension());
        (implied != Some(self.format)).then_some(self.format)
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
        ImportPath::new_with_build_file_cells(
            cell_path("root", filename),
            BuildFileCell::new(CellName::testing_new("root")),
        )
    }

    /// A `load()` carrying an `?as=` hint.
    fn import_as(filename: &str, format: ImportFormat) -> buck2_error::Result<ImportPath> {
        ImportPath::new_with_explicit_format(
            cell_path("root", filename),
            BuildFileCell::new(CellName::testing_new("root")),
            format,
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

            let explicit = import_as(filename, ImportFormat::Toml).unwrap();
            assert_eq!(ImportFormat::Toml, explicit.format(), "{filename}");
            assert_eq!(
                Some(ImportFormat::Toml),
                explicit.format_override(),
                "{filename}"
            );
        }
    }

    #[test]
    fn an_extensionless_file_keeps_a_redundant_looking_format() {
        // `Makefile` has no extension for `Bzl` to be redundant with, so the
        // explicit format is the only thing naming one. It is still not an
        // override: `?as=bzl` cannot be written, so nothing may render it.
        let explicit = import_as("Makefile", ImportFormat::Bzl).unwrap();
        assert_eq!(ImportFormat::Bzl, explicit.format());
        assert_eq!(None, explicit.format_override());
    }

    #[test]
    fn an_lsp_path_never_renders_an_as_bzl_hint() {
        for filename in ["BUCK", "a.json"] {
            let lsp = ImportPath::new_hack_for_lsp(
                cell_path("root", filename),
                BuildFileCell::new(CellName::testing_new("root")),
            )
            .unwrap();
            assert_eq!(ImportFormat::Bzl, lsp.format(), "{filename}");
            assert_eq!(None, lsp.format_override(), "{filename}");
            assert_eq!(format!("root//pkg/{filename}"), lsp.to_string());
        }
    }

    #[test]
    fn explicit_format_overrides_the_extension() {
        let as_toml = import_as("a.json", ImportFormat::Toml).unwrap();
        assert_eq!(ImportFormat::Toml, as_toml.format());
        // The same file under two formats must stay two distinct modules.
        assert_ne!(import("a.json").unwrap(), as_toml);
    }

    #[test]
    fn redundant_format_collapses_so_the_module_is_not_duplicated() {
        // A hint restating the extension must leave no trace, so `:a.toml?as=toml`
        // and `:a.toml` are one module. Two `ImportPath`s are two `EvalImportKey`s,
        // i.e. the file read and parsed twice, handing out two copies of itself.
        for (filename, format) in [
            ("a.bzl", ImportFormat::Bzl),
            ("a.json", ImportFormat::Json),
            ("a.toml", ImportFormat::Toml),
        ] {
            let explicit = import_as(filename, format).unwrap();
            assert_eq!(import(filename).unwrap(), explicit, "{filename}");
            assert_eq!(None, explicit.format_override(), "{filename}");
            assert_eq!(format, explicit.format(), "{filename}");
        }
    }

    #[test]
    fn display_round_trips_an_explicit_format() {
        assert_eq!("root//pkg/a.bzl", import("a.bzl").unwrap().to_string());
        assert_eq!(
            "root//pkg/Cargo.lock?as=toml",
            import_as("Cargo.lock", ImportFormat::Toml)
                .unwrap()
                .to_string()
        );
    }

    #[test]
    fn the_alternate_display_is_the_file_alone() {
        let hinted = import_as("Cargo.lock", ImportFormat::Toml).unwrap();
        assert_eq!("root//pkg/Cargo.lock", format!("{hinted:#}"));

        let cross_cell = ImportPath::new_with_explicit_format(
            cell_path("root", "Cargo.lock"),
            BuildFileCell::new(CellName::testing_new("other")),
            ImportFormat::Toml,
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
        assert!(import_as("a.bzl?as=toml", ImportFormat::Toml).is_err());
    }
}
