/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::hash::Hash;

use allocative::Allocative;
use buck2_core::package::PackageLabel;
use derive_more::Display;
use derive_more::From;
use dupe::Dupe;
use serde::Serialize;
use starlark::any::ProvidesStaticType;
use starlark::collections::StarlarkHasher;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::starlark_value;

#[derive(
    Clone,
    Dupe,
    Debug,
    Hash,
    Display,
    PartialEq,
    Eq,
    From,
    ProvidesStaticType,
    Serialize,
    Allocative,
    StarlarkPagable
)]
#[serde(transparent)]
pub struct StarlarkPackagePath {
    #[starlark_pagable(pagable)]
    pkg: PackageLabel,
}

starlark_simple_value!(StarlarkPackagePath);

impl StarlarkPackagePath {
    pub fn new(pkg: PackageLabel) -> Self {
        StarlarkPackagePath { pkg }
    }

    pub fn pkg(&self) -> &PackageLabel {
        &self.pkg
    }
}

starlark::methods_static!(PACKAGE_PATH_METHODS = package_path_methods);

#[starlark_value(type = "PackagePath")]
impl<'v> StarlarkValue<'v> for StarlarkPackagePath {
    fn get_methods() -> Option<&'static Methods> {
        Some(PACKAGE_PATH_METHODS.methods())
    }

    fn write_hash(this: &Self, hasher: &mut StarlarkHasher) {
        this.hash(hasher);
    }

    fn equals(this: &Self, other: &Self) -> bool {
        this.pkg == other.pkg
    }

    fn compare(this: &Self, other: &Self) -> std::cmp::Ordering {
        this.pkg.cmp(&other.pkg)
    }
}

/// A PackagePath represents a Buck package like package `root//foo/bar`
#[starlark_module]
fn package_path_methods(builder: &mut MethodsBuilder) {
    /// Returns the name of the cell of the package
    ///
    /// # Examples
    ///
    /// ```python
    /// # For a package (root//foo/bar):
    /// package_path.cell  # Returns: "root"
    /// ```
    #[starlark(attribute)]
    fn cell<'v>(this: &'v StarlarkPackagePath) -> starlark::Result<&'v str> {
        Ok(this.pkg.cell_name().as_str())
    }
}

#[starlark_module]
#[starlark_types(StarlarkPackagePath as PackagePath)]
pub fn register_package_path(globals: &mut GlobalsBuilder) {}
