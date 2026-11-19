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
use std::hash::Hasher;

use allocative::Allocative;
use buck2_error::BuckErrorSerde;
use buck2_hash::BuckHasher;
use derive_more::Display;
use dupe::Dupe;
use equivalent::Equivalent;
use pagable::Pagable;
use serde::Deserialize;
use serde::Serialize;
use static_interner::Intern;
use static_interner::interner;
use strong_hash::StrongHash;

#[derive(Debug, buck2_error::Error)]
#[buck2(input)]
enum CellNameError {
    #[error("Cell name must be non-empty")]
    Empty,
}

#[derive(
    Clone, Debug, Display, Eq, PartialEq, Ord, PartialOrd, Allocative, Pagable
)]
struct CellNameData(Box<str>);

#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for CellNameData {
    fn hash<H: Hasher>(&self, state: &mut H) {
        CellNameDataRef(&self.0).hash(state)
    }
}

impl StrongHash for CellNameData {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        CellNameDataRef(&self.0).strong_hash(state)
    }
}

#[derive(Clone, Debug, Display, Hash, Eq, PartialEq, StrongHash)]
struct CellNameDataRef<'a>(&'a str);

impl Equivalent<CellNameData> for CellNameDataRef<'_> {
    fn equivalent(&self, key: &CellNameData) -> bool {
        self.0 == &*key.0
    }
}

impl<'a> From<CellNameDataRef<'a>> for CellNameData {
    fn from(d: CellNameDataRef<'a>) -> Self {
        CellNameData(d.0.into())
    }
}

interner!(INTERNER, BuckHasher, CellNameData);

/// A 'CellName' is a canonicalized, human-readable name that corresponds to a
/// 'CellInstance'. There should be a one to one mapping between a 'CellName'
/// and a 'CellInstance'.
///
/// The cell within a fully qualified target like `foo//some:target` is `foo`.
/// The cell name is also restricted to alphabet characters (i.e. shouldn't
/// contain any special characters like `/`), so `foo/bar//some:target` has an
/// invalid cell name of `foo/bar`.
#[derive(
    Clone, Dupe, Copy, Debug, Display, Hash, Eq, PartialEq, Ord, PartialOrd, Allocative,
    StrongHash, Pagable
)]
pub struct CellName(Intern<CellNameData>);

impl CellName {
    /// Construct a cell name.
    ///
    /// This function is unchecked because it does not validate that the cell points
    /// to an existing cell. This function should only be used when creating
    /// repository cells at startup.
    pub fn unchecked_new(name: &str) -> buck2_error::Result<CellName> {
        if name.is_empty() {
            return Err(CellNameError::Empty.into());
        }
        Ok(CellName(INTERNER.intern(CellNameDataRef(name))))
    }

    pub fn testing_new(name: &str) -> CellName {
        CellName::unchecked_new(name).unwrap()
    }

    pub fn as_str(&self) -> &'static str {
        &self.0.deref_static().0
    }
}

impl Serialize for CellName {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.as_str().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CellName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = String::deserialize(deserializer)?;
        CellName::unchecked_new(&v).serde_err()
    }
}
