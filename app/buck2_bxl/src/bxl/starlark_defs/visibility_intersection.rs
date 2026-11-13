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
use buck2_node::visibility::VisibilityIntersectionLayer;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::starlark_value;

/// One layer of a package's visibility intersection, as returned by
/// `bxl.read_package_visibility_intersection`.
#[derive(
    Clone,
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    starlark::StarlarkPagableUnsupported
)]
#[display("bxl.VisibilityIntersectionLayer({})", _0.to_json())]
pub(crate) struct StarlarkVisibilityIntersectionLayer(pub(crate) VisibilityIntersectionLayer);

#[starlark_module]
fn visibility_intersection_layer_methods(builder: &mut MethodsBuilder) {
    /// The layer's `package(visibility=...)` patterns, in the same shape as
    /// `bxl.read_package_visibility`.
    #[starlark(attribute)]
    fn patterns<'v>(
        this: &StarlarkVisibilityIntersectionLayer,
        heap: Heap<'v>,
    ) -> starlark::Result<Value<'v>> {
        Ok(heap.alloc(this.0.patterns().to_json()).to_value())
    }

    /// Package patterns whose targets skip this layer.
    #[starlark(attribute)]
    fn exempt_targets(this: &StarlarkVisibilityIntersectionLayer) -> starlark::Result<Vec<String>> {
        Ok(this
            .0
            .exemptions()
            .iter()
            .map(ToString::to_string)
            .collect())
    }
}

starlark_simple_value!(StarlarkVisibilityIntersectionLayer);

starlark::methods_static!(
    VISIBILITY_INTERSECTION_LAYER_METHODS = visibility_intersection_layer_methods
);

#[starlark_value(type = "bxl.VisibilityIntersectionLayer")]
impl<'v> StarlarkValue<'v> for StarlarkVisibilityIntersectionLayer {
    fn get_methods() -> Option<&'static Methods> {
        Some(VISIBILITY_INTERSECTION_LAYER_METHODS.methods())
    }
}
