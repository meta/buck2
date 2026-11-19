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
use buck2_core::cells::CellAliasResolver;
use buck2_core::cells::CellResolver;
use buck2_core::cells::build_file_cell::BuildFileCell;
use pagable::Pagable;

#[derive(Clone, Debug, Allocative, Pagable)]
pub struct InterpreterCellInfo {
    cell_name: BuildFileCell,
    cell_resolver: CellResolver,
    cell_alias_resolver: CellAliasResolver,
}

impl InterpreterCellInfo {
    pub(crate) fn new(
        cell_name: BuildFileCell,
        cell_resolver: CellResolver,
        cell_alias_resolver: CellAliasResolver,
    ) -> buck2_error::Result<Self> {
        Ok(Self {
            cell_name,
            cell_resolver,
            cell_alias_resolver,
        })
    }

    pub(crate) fn name(&self) -> BuildFileCell {
        self.cell_name
    }

    pub fn cell_resolver(&self) -> &CellResolver {
        &self.cell_resolver
    }

    pub fn cell_alias_resolver(&self) -> &CellAliasResolver {
        &self.cell_alias_resolver
    }
}
