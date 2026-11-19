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
use derivative::Derivative;
use dupe::Dupe;
use pagable::Pagable;

use crate::cells::cell_root_path::CellRootPath;
use crate::cells::cell_root_path::CellRootPathBuf;
use crate::cells::external::ExternalCellOrigin;
use crate::cells::name::CellName;
use crate::cells::nested::NestedCells;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum CellInstanceError {
    #[error(
        "Attempted to refer to cell `{0}`; however, this is an external cell which cannot be used from `{1}`"
    )]
    ExpectedNonExternalCell(CellName, &'static str),
    #[error("External cell `{0}` cannot have a nested cell `{1}`")]
    NestedInExternalCell(CellName, CellName),
}

/// A 'CellInstance', contains a 'CellName' and a path for that cell.
#[derive(
    Clone,
    Debug,
    derive_more::Display,
    Dupe,
    PartialEq,
    Eq,
    Allocative,
    Pagable
)]
#[display("{}", _0.name)]
pub struct CellInstance(Arc<CellData>);

#[derive(Derivative, PartialEq, Eq, Allocative, Pagable)]
#[derivative(Debug)]
struct CellData {
    /// the fully canonicalized 'CellName'
    name: CellName,
    /// the project relative path to this 'CellInstance'
    path: CellRootPathBuf,
    external: Option<ExternalCellOrigin>,
    nested_cells: NestedCells,
}

impl CellInstance {
    pub fn new(
        name: CellName,
        path: CellRootPathBuf,
        external: Option<ExternalCellOrigin>,
        nested_cells: NestedCells,
    ) -> buck2_error::Result<CellInstance> {
        if external.is_some()
            && let Some(nested) = nested_cells.check_empty()
        {
            return Err(CellInstanceError::NestedInExternalCell(name, nested).into());
        }
        Ok(CellInstance(Arc::new(CellData {
            name,
            path,
            external,
            nested_cells,
        })))
    }

    /// Get the name of the cell, as supplied in `cell_name//foo:bar`.
    #[inline]
    pub fn name(&self) -> CellName {
        self.0.name.dupe()
    }

    /// Get the path of the cell, where it is routed.
    #[inline]
    pub fn path(&self) -> &CellRootPath {
        &self.0.path
    }

    #[inline]
    pub fn nested_cells(&self) -> &NestedCells {
        &self.0.nested_cells
    }

    #[inline]
    pub fn external(&self) -> Option<&ExternalCellOrigin> {
        self.0.external.as_ref()
    }

    #[inline]
    pub fn expect_non_external(&self, context: &'static str) -> buck2_error::Result<()> {
        match self.0.external {
            Some(_) => Err(CellInstanceError::ExpectedNonExternalCell(self.name(), context).into()),
            None => Ok(()),
        }
    }
}
