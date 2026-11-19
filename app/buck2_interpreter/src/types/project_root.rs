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
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::static_starlark_value;
use starlark::values::AllocFrozenRepresentation;
use starlark::values::AllocRepresentation;
use starlark::values::Allocator;
use starlark::values::FrozenAllocator;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;

#[derive(
    Debug,
    PartialEq,
    Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
pub struct StarlarkProjectRoot;

#[starlark_value(type = "ProjectRoot", alloc = by_hand)]
impl<'v> StarlarkValue<'v> for StarlarkProjectRoot {}

static_starlark_value!(STARLARK_PROJECT_ROOT: StarlarkProjectRoot = StarlarkProjectRoot);

impl<'v> AllocRepresentation<'v> for StarlarkProjectRoot {
    type Repr = Self;

    fn alloc(self, _allocator: Allocator<'v>) -> ValueTyped<'v, Self> {
        STARLARK_PROJECT_ROOT.at()
    }
}

impl<'fv> AllocFrozenRepresentation<'fv> for StarlarkProjectRoot {
    type Repr = Self;

    fn alloc_frozen(self, _allocator: FrozenAllocator<'fv>) -> ValueTyped<'fv, Self> {
        STARLARK_PROJECT_ROOT.at()
    }
}

#[starlark_module]
#[starlark_types(StarlarkProjectRoot as ProjectRoot)]
pub fn register_project_root(globals: &mut GlobalsBuilder) {}
