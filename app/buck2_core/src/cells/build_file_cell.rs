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
use dupe::Dupe;
use pagable::Pagable;
use strong_hash::StrongHash;

use crate::cells::name::CellName;

/// Used to hold the cell name for the top-level build file being interpreted.
/// There's a subtlety where that doesn't necessarily match the cell of the file
/// being interpreted. This gets its own type because its easy to get wrong.
#[derive(
    Clone,
    Dupe,
    Copy,
    Hash,
    Eq,
    PartialEq,
    Ord,
    PartialOrd,
    Debug,
    derive_more::Display,
    Allocative,
    StrongHash,
    Pagable
)]
#[repr(C)]
pub struct BuildFileCell(CellName);

impl BuildFileCell {
    #[inline]
    pub fn new(name: CellName) -> Self {
        Self(name)
    }

    #[inline]
    pub fn name(&self) -> CellName {
        self.0.dupe()
    }
}
