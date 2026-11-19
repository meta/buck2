/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Borrow;

use allocative::Allocative;
use pagable::Pagable;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum CellAliasError {
    #[error("Empty alias where non-empty is required")]
    EmptyAlias,
}

/// A 'CellAlias' is a user-provided string name that maps to a 'CellName'.
/// The mapping of 'CellAlias' to 'CellName' is specific to the current cell so
/// that the same 'CellAlias' may map to different 'CellName's depending on what
/// the current 'CellInstance' is that references the 'CellAlias'.
#[derive(
    Clone,
    Debug,
    derive_more::Display,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Allocative
)]
pub struct CellAlias(String);

impl CellAlias {
    pub fn new(alias: String) -> CellAlias {
        CellAlias(alias)
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for CellAlias {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// Empty string is an alias for the current cell.
/// This type does not permit it.
#[derive(
    derive_more::Display,
    Debug,
    Clone,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    Pagable
)]
pub struct NonEmptyCellAlias(String);

impl NonEmptyCellAlias {
    pub fn new(alias: String) -> buck2_error::Result<NonEmptyCellAlias> {
        if alias.is_empty() {
            Err(CellAliasError::EmptyAlias.into())
        } else {
            Ok(NonEmptyCellAlias(alias))
        }
    }

    pub fn testing_new(alias: &str) -> NonEmptyCellAlias {
        Self::new(alias.to_owned()).unwrap()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for NonEmptyCellAlias {
    fn borrow(&self) -> &str {
        &self.0
    }
}
