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
use buck2_core::bzl::ImportPath;
use buck2_core::cells::CellResolver;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use buck2_core::cells::paths::CellRelativePath;
use pagable::Pagable;

#[derive(Debug, derive_more::Display, Clone, Eq, PartialEq, Allocative, Pagable)]
pub struct PreludePath(ImportPath);

impl PreludePath {
    #[inline]
    pub fn import_path(&self) -> &ImportPath {
        &self.0
    }

    #[inline]
    pub fn prelude_cell(&self) -> CellName {
        self.import_path().cell()
    }

    pub fn is_prelude_path(&self, import_path: &CellPath) -> bool {
        import_path.starts_with(self.0.path_parent())
    }

    pub fn testing_new(import_path: ImportPath) -> PreludePath {
        PreludePath(import_path)
    }
}

pub fn prelude_path(cell_resolver: &CellResolver) -> buck2_error::Result<Option<PreludePath>> {
    let alias_resolver = cell_resolver.root_cell_cell_alias_resolver();
    let Ok(prelude_cell) = alias_resolver.resolve("prelude") else {
        return Ok(None);
    };
    let prelude_file = CellRelativePath::unchecked_new("prelude.bzl");
    Ok(Some(PreludePath(ImportPath::new_same_cell(
        CellPath::new(prelude_cell, prelude_file),
    )?)))
}
