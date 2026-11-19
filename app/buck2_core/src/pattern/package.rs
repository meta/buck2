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
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::cells::cell_path::CellPath;
use crate::package::PackageLabel;

#[derive(
    Clone,
    Debug,
    Eq,
    Hash,
    StrongHash,
    PartialEq,
    Allocative,
    Pagable,
    derive_more::Display
)]
pub enum PackagePattern {
    #[display("{}:", _0)]
    Package(PackageLabel),
    #[display("{}{}...", _0, if _0.path().is_empty() { "" } else { "/" })]
    Recursive(CellPath),
}

impl PackagePattern {
    pub fn matches(&self, package: PackageLabel) -> bool {
        match self {
            PackagePattern::Package(pattern) => pattern == &package,
            PackagePattern::Recursive(cell_path) => package.as_cell_path().starts_with(*cell_path),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Allocative, Pagable)]
pub enum PackagePredicate {
    Any,
    AnyOf(Vec<PackagePattern>),
}

impl PackagePredicate {
    pub fn matches(&self, package: PackageLabel) -> bool {
        match self {
            PackagePredicate::Any => true,
            PackagePredicate::AnyOf(patterns) => {
                patterns.iter().any(|pattern| pattern.matches(package))
            }
        }
    }
}
