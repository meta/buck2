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
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;

use crate::bzl_global::BzlGlobalId;
use crate::cells::cell_path::CellPath;

/// An execution group declared with `exec_group()`.
#[derive(
    Clone,
    Dupe,
    Debug,
    Display,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative,
    strong_hash::StrongHash,
    Pagable
)]
pub struct NamedExecutionGroup(BzlGlobalId);

impl NamedExecutionGroup {
    pub fn new(name: String, cell: CellPath) -> Self {
        Self(BzlGlobalId::new(name, cell))
    }

    pub fn name(&self) -> &str {
        self.0.name()
    }

    /// The `.bzl` file that defines the group.
    pub fn defining_file(&self) -> CellPath {
        self.0.defining_file()
    }

    /// The group in `declared` that has this group's name but is a different group, if any.
    pub fn same_name_in<'a>(
        &self,
        declared: impl IntoIterator<Item = &'a NamedExecutionGroup>,
    ) -> Option<&'a NamedExecutionGroup> {
        declared
            .into_iter()
            .find(|d| d.name() == self.name() && *d != self)
    }
}

/// The execution group an action or exec dep belongs to.
#[derive(Clone, Dupe, Debug, Display, Eq, PartialEq, Hash, Allocative, Pagable)]
pub enum ExecutionGroup {
    /// The group every rule has; its platform is the target's execution platform.
    #[display("DefaultExecGroup")]
    Default,
    #[display("{_0}")]
    Named(NamedExecutionGroup),
}
