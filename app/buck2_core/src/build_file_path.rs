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
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use dupe::Dupe;
use pagable::Pagable;

use crate::cells::build_file_cell::BuildFileCell;
use crate::cells::cell_path::CellPath;
use crate::cells::name::CellName;
use crate::package::PackageLabel;

/// Path of a build file (e.g. `BUCK`) only. (`bzl` files are not included).
#[derive(
    Clone,
    Hash,
    Eq,
    PartialEq,
    Debug,
    derive_more::Display,
    Allocative,
    Pagable
)]
#[display("{}:{}", package, filename)]
pub struct BuildFilePath {
    /// The package of this build file
    package: PackageLabel,
    /// The build file's filename (which can be configured). i.e. `BUCK`
    filename: FileNameBuf,
}

impl BuildFilePath {
    pub fn new(package: PackageLabel, filename: FileNameBuf) -> Self {
        Self { package, filename }
    }

    pub fn testing_new(path: &str) -> Self {
        let (package, filename) = path.split_once(':').unwrap();
        Self::new(
            PackageLabel::testing_parse(package),
            FileNameBuf::try_from(filename.to_owned()).unwrap(),
        )
    }

    pub fn cell(&self) -> CellName {
        self.package.cell_name()
    }

    pub fn package(&self) -> PackageLabel {
        self.package.dupe()
    }

    pub fn path(&self) -> CellPath {
        self.package.as_cell_path().join(&self.filename)
    }

    pub fn build_file_cell(&self) -> BuildFileCell {
        BuildFileCell::new(self.cell())
    }

    pub fn filename(&self) -> &FileName {
        &self.filename
    }
}

#[cfg(test)]
mod tests {
    use crate::build_file_path::BuildFilePath;

    #[test]
    fn test_testing_new() {
        // `testing_new` accepts the same format `Display` produces.
        assert_eq!(
            "foo//bar/baz:BUCK",
            BuildFilePath::testing_new("foo//bar/baz:BUCK").to_string()
        );
    }
}
