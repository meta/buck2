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
use buck2_core::cells::cell_path::CellPath;
use buck2_core::cells::paths::CellRelativePath;
use derive_more::Display;
use pagable::Pagable;
use serde::Serialize;
use serde::Serializer;
use starlark::any::ProvidesStaticType;
use starlark::collections::StarlarkHasher;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::starlark_value;

#[derive(
    Debug,
    PartialEq,
    Display,
    ProvidesStaticType,
    Allocative,
    Pagable,
    StarlarkPagable
)]
pub struct StarlarkCellPath(#[starlark_pagable(pagable)] pub CellPath);

starlark_simple_value!(StarlarkCellPath);

impl Serialize for StarlarkCellPath {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.collect_str(self)
    }
}

starlark::methods_static!(CELL_PATH_METHODS = cell_path_methods);

#[starlark_value(type = "CellPath")]
impl<'v> StarlarkValue<'v> for StarlarkCellPath {
    fn get_methods() -> Option<&'static Methods> {
        Some(CELL_PATH_METHODS.methods())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        match other.downcast_ref::<StarlarkCellPath>() {
            None => Ok(false),
            Some(v) => Ok(v.0 == self.0),
        }
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.0.hash(hasher);
        Ok(())
    }
}

/// A [cell](../../../concepts/key_concepts/#cells) and path relative to the cell.
///
/// `CellPath`s are seen in labels like [`TargetLabel`](../TargetLabel) and
/// [`ProvidersLabel`](../ProvidersLabel).
#[starlark_module]
fn cell_path_methods(builder: &mut MethodsBuilder) {
    /// Create a new `CellPath` by joining a path to this path.
    ///
    /// The path components are
    /// [normalized](https://docs.rs/relative-path/1.9.3/relative_path/struct.RelativePath.html#method.normalize),
    /// e.g. `puppy/../doggy` will become `doggy`.
    fn add<'v>(
        this: &StarlarkCellPath,
        arg: &str,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkCellPath>> {
        // FIXME(JakobDegen): `join_normalized` is always wrong
        let p = this
            .0
            .path()
            .as_forward_relative_path()
            .join_normalized(arg)?;
        let p = CellPath::new(this.0.cell(), CellRelativePath::new(&p));
        Ok(ValueOf::alloc(heap, StarlarkCellPath(p)))
    }
}

#[starlark_module]
#[starlark_types(StarlarkCellPath as CellPath)]
pub fn register_cell_path(globals: &mut GlobalsBuilder) {}
