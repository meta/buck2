/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

pub mod package_relative_path;
pub mod source_path;

use allocative::Allocative;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckHasher;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use static_interner::Intern;
use static_interner::interner;
use strong_hash::StrongHash;

use crate::cells::cell_path::CellPath;
use crate::cells::name::CellName;
use crate::cells::paths::CellRelativePath;
use crate::pattern::pattern::Modifiers;

/// A label representing a package.
///
/// This is the same data as a `CellPath`, but carries additional expectations/semantics associated
/// with being treated as a package.
#[derive(
    Copy, Clone, Dupe, Debug, Display, Eq, PartialEq, Hash, Ord, PartialOrd, Allocative,
    StrongHash, Pagable
)]
// FIXME(JakobDegen): Whether this interning makes sense has not been re-evaluated since `CellPath`
// itself became interned-ish and `Copy`
pub struct PackageLabel(Intern<PackageLabelData>);

impl Serialize for PackageLabel {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.to_string())
    }
}

#[derive(Dupe, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct PackageLabelWithModifiers {
    pub package: PackageLabel,
    pub modifiers: Modifiers,
}

#[derive(
    Debug, Display, Eq, PartialEq, Hash, Ord, PartialOrd, Allocative, Pagable, StrongHash
)]
struct PackageLabelData(CellPath);

interner!(INTERNER, BuckHasher, PackageLabelData);

impl PackageLabel {
    #[inline]
    pub fn new(cell: CellName, path: &CellRelativePath) -> buck2_error::Result<Self> {
        PackageLabel::from_cell_path(CellPath::new(cell, path))
    }

    #[inline]
    pub fn from_cell_path(path: CellPath) -> buck2_error::Result<Self> {
        Ok(PackageLabel(INTERNER.intern(PackageLabelData(path))))
    }

    #[inline]
    pub fn cell_name(&self) -> CellName {
        self.0.0.cell()
    }

    #[inline]
    pub fn cell_relative_path(&self) -> &'static CellRelativePath {
        self.0.deref_static().0.path()
    }

    #[inline]
    pub fn to_cell_path(&self) -> CellPath {
        self.0.0
    }

    #[inline]
    pub fn as_cell_path(&self) -> CellPath {
        self.0.0
    }

    pub fn join(&self, path: &ForwardRelativePath) -> buck2_error::Result<Self> {
        if path.is_empty() {
            Ok(self.dupe())
        } else {
            PackageLabel::from_cell_path(self.as_cell_path().join(path))
        }
    }

    pub fn parent(&self) -> buck2_error::Result<Option<PackageLabel>> {
        match self.as_cell_path().parent() {
            Some(parent) => PackageLabel::from_cell_path(parent).map(Some),
            None => Ok(None),
        }
    }

    // Following functions should only be used in tests, so they have "testing" in their names.

    /// Some package name usable in tests.
    pub fn testing() -> PackageLabel {
        PackageLabel::new(
            CellName::testing_new("root"),
            CellRelativePath::new(ForwardRelativePath::new("package/subdir").unwrap()),
        )
        .unwrap()
    }

    pub fn testing_new(cell: &str, path: &str) -> PackageLabel {
        PackageLabel::new(
            CellName::testing_new(cell),
            CellRelativePath::new(ForwardRelativePath::new(path).unwrap()),
        )
        .unwrap()
    }

    pub fn testing_parse(label: &str) -> PackageLabel {
        let (cell, path) = label.split_once("//").unwrap();
        PackageLabel::testing_new(cell, path)
    }
}

#[cfg(test)]
mod tests {
    use crate::package::PackageLabel;

    #[test]
    fn test_serialize() {
        assert_eq!(
            "foo//bar/baz",
            PackageLabel::testing_parse("foo//bar/baz").to_string()
        );
    }
}
