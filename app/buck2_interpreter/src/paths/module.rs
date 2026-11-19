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
use buck2_core::bxl::BxlFilePath;
use buck2_core::bzl::ImportPath;
use buck2_core::cells::build_file_cell::BuildFileCell;
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::name::CellName;
use dupe::Dupe;
use pagable::Pagable;
use starlark::collections::Equivalent;
use strong_hash::StrongHash;

use crate::paths::path::OwnedStarlarkPath;
use crate::paths::path::StarlarkPath;

/// Path to file containing starlark that can be evaluated as a loaded module by the interpreter.
#[derive(
    derive_more::Display,
    Clone,
    Copy,
    Dupe,
    Debug,
    Hash,
    StrongHash,
    Eq,
    PartialEq
)]
pub enum StarlarkModulePath<'a> {
    /// a file to be imported
    LoadFile(&'a ImportPath),
    /// a bxl file to be evaluated
    BxlFile(&'a BxlFilePath),
    /// a json file to be parsed
    JsonFile(&'a ImportPath),
    /// a toml file to be parsed
    TomlFile(&'a ImportPath),
}

impl Equivalent<OwnedStarlarkModulePath> for StarlarkModulePath<'_> {
    fn equivalent(&self, key: &OwnedStarlarkModulePath) -> bool {
        *self == key.borrow()
    }
}

impl<'a> StarlarkModulePath<'a> {
    pub fn unpack_load_file(&self) -> Option<&'a ImportPath> {
        match self {
            Self::LoadFile(p) => Some(p),
            _ => None,
        }
    }

    pub fn cell(&self) -> CellName {
        match self {
            StarlarkModulePath::LoadFile(l) => l.cell(),
            StarlarkModulePath::BxlFile(b) => b.cell(),
            StarlarkModulePath::JsonFile(j) => j.cell(),
            StarlarkModulePath::TomlFile(t) => t.cell(),
        }
    }

    pub fn build_file_cell(&self) -> BuildFileCell {
        match self {
            StarlarkModulePath::LoadFile(l) => l.build_file_cell(),
            StarlarkModulePath::BxlFile(b) => b.build_file_cell(),
            StarlarkModulePath::JsonFile(j) => j.build_file_cell(),
            StarlarkModulePath::TomlFile(t) => t.build_file_cell(),
        }
    }

    pub fn path(&self) -> &CellPath {
        match self {
            StarlarkModulePath::LoadFile(l) => l.path(),
            StarlarkModulePath::BxlFile(b) => b.path(),
            StarlarkModulePath::JsonFile(j) => j.path(),
            StarlarkModulePath::TomlFile(t) => t.path(),
        }
    }

    pub fn starlark_path(&self) -> StarlarkPath<'_> {
        match self {
            StarlarkModulePath::LoadFile(l) => StarlarkPath::LoadFile(l),
            StarlarkModulePath::BxlFile(b) => StarlarkPath::BxlFile(b),
            StarlarkModulePath::JsonFile(j) => StarlarkPath::JsonFile(j),
            StarlarkModulePath::TomlFile(t) => StarlarkPath::TomlFile(t),
        }
    }

    pub fn to_owned(&self) -> OwnedStarlarkModulePath {
        OwnedStarlarkModulePath::new(*self)
    }
}

#[derive(Clone, derive_more::Display, Debug, Eq, PartialEq, Allocative, Pagable)]
#[display("{}", self.borrow())]
pub enum OwnedStarlarkModulePath {
    LoadFile(ImportPath),
    BxlFile(BxlFilePath),
    JsonFile(ImportPath),
    TomlFile(ImportPath),
}

#[allow(clippy::derived_hash_with_manual_eq)]
impl Hash for OwnedStarlarkModulePath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.borrow().hash(state)
    }
}

impl StrongHash for OwnedStarlarkModulePath {
    fn strong_hash<H: Hasher>(&self, state: &mut H) {
        self.borrow().strong_hash(state)
    }
}

impl OwnedStarlarkModulePath {
    pub fn new(path: StarlarkModulePath<'_>) -> Self {
        match path {
            StarlarkModulePath::LoadFile(p) => Self::LoadFile(p.clone()),
            StarlarkModulePath::BxlFile(p) => Self::BxlFile(p.clone()),
            StarlarkModulePath::JsonFile(p) => Self::JsonFile(p.clone()),
            StarlarkModulePath::TomlFile(t) => Self::TomlFile(t.clone()),
        }
    }

    pub fn borrow(&self) -> StarlarkModulePath<'_> {
        match self {
            OwnedStarlarkModulePath::LoadFile(p) => StarlarkModulePath::LoadFile(p),
            OwnedStarlarkModulePath::BxlFile(p) => StarlarkModulePath::BxlFile(p),
            OwnedStarlarkModulePath::JsonFile(p) => StarlarkModulePath::JsonFile(p),
            OwnedStarlarkModulePath::TomlFile(p) => StarlarkModulePath::TomlFile(p),
        }
    }

    pub fn path(&self) -> CellPath {
        match self {
            OwnedStarlarkModulePath::LoadFile(p) => *p.path(),
            OwnedStarlarkModulePath::BxlFile(p) => *p.path(),
            OwnedStarlarkModulePath::JsonFile(p) => *p.path(),
            OwnedStarlarkModulePath::TomlFile(p) => *p.path(),
        }
    }

    pub fn into_starlark_path(self) -> OwnedStarlarkPath {
        match self {
            OwnedStarlarkModulePath::LoadFile(l) => OwnedStarlarkPath::LoadFile(l),
            OwnedStarlarkModulePath::BxlFile(b) => OwnedStarlarkPath::BxlFile(b),
            OwnedStarlarkModulePath::JsonFile(b) => OwnedStarlarkPath::JsonFile(b),
            OwnedStarlarkModulePath::TomlFile(t) => OwnedStarlarkPath::TomlFile(t),
        }
    }
}
