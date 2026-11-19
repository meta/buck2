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
use buck2_hash::BuckHasher;
use derive_more::Display;
use dupe::Dupe;
use pagable::Pagable;
use static_interner::Intern;
use static_interner::interner;

use crate::cells::cell_path::CellPath;

#[derive(
    Clone,
    Pagable,
    strong_hash::StrongHash,
    Debug,
    Display,
    Eq,
    PartialEq,
    Hash,
    Ord,
    PartialOrd,
    Allocative
)]
#[display("{name}")]
struct BzlGlobalIdInner {
    /// The name of the global the value was assigned to.
    name: String,
    /// The `.bzl` file that assigned it.
    cell: CellPath,
}

impl<'a> From<&'a BzlGlobalIdInner> for BzlGlobalIdInner {
    fn from(x: &'a BzlGlobalIdInner) -> Self {
        x.clone()
    }
}

/// The identity of a value that is named by the `.bzl` global it is assigned to, such as a
/// `plugins.kind()` or an `exec_group()`. Each call that creates such a value gets a distinct
/// identity, even when two values share a name.
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
pub struct BzlGlobalId(Intern<BzlGlobalIdInner>);

interner!(BZL_GLOBAL_ID_INTERNER, BuckHasher, BzlGlobalIdInner);

impl BzlGlobalId {
    pub fn new(name: String, cell: CellPath) -> Self {
        // FIXME(JakobDegen): Interning is overkill here, this is never called with the same
        // arguments twice. However, we do want pointer equality and pre-hashing, is there an easier
        // way to get that?
        Self(BZL_GLOBAL_ID_INTERNER.intern(BzlGlobalIdInner { name, cell }))
    }

    pub fn name(&self) -> &str {
        &self.0.deref_static().name
    }

    /// The `.bzl` file that defines the value.
    pub fn defining_file(&self) -> CellPath {
        self.0.deref_static().cell
    }
}
