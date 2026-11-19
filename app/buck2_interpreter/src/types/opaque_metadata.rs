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
use starlark::any::ProvidesStaticType;
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

/// We do not make metadata available to rules, so instead we expose this opaque value when trying
/// to resolve it to a Starlark object.
#[derive(
    Debug,
    Dupe,
    Clone,
    PartialEq,
    ProvidesStaticType,
    Allocative,
    NoSerialize,
    Display,
    StarlarkPagable
)]
#[display("{:?}", self)]
pub struct OpaqueMetadata;

#[starlark_value(type = "OpaqueMetadata", alloc = by_hand)]
impl<'v> StarlarkValue<'v> for OpaqueMetadata {}

static_starlark_value!(OPAQUE_METADATA: OpaqueMetadata = OpaqueMetadata);

impl<'v> AllocRepresentation<'v> for OpaqueMetadata {
    type Repr = Self;

    fn alloc(self, _allocator: Allocator<'v>) -> ValueTyped<'v, Self> {
        OPAQUE_METADATA.at()
    }
}

impl<'fv> AllocFrozenRepresentation<'fv> for OpaqueMetadata {
    type Repr = Self;

    fn alloc_frozen(self, _allocator: FrozenAllocator<'fv>) -> ValueTyped<'fv, Self> {
        OPAQUE_METADATA.at()
    }
}
