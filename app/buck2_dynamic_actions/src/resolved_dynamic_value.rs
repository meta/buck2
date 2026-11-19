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
use buck2_build_api::interpreter::rule_defs::provider::collection::ProviderCollection;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_complex_value;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::FrozenValueTyped;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    NoSerialize,
    ProvidesStaticType,
    StarlarkPagable,
    Trace,
    Freeze
)]
#[display("ResolvedDynamicValue<{}>", self.value)]
pub struct StarlarkResolvedDynamicValue<'v> {
    pub value: FrozenValueTyped<'v, ProviderCollection<'v>>,
}

starlark_complex_value!(pub StarlarkResolvedDynamicValue);

starlark::methods_static!(RESOLVED_DYNAMIC_VALUE_METHODS = resolved_dynamic_value_methods);

#[starlark_value(type = "ResolvedDynamicValue")]
impl<'v> StarlarkValue<'v> for StarlarkResolvedDynamicValue<'v> {
    fn get_methods() -> Option<&'static Methods> {
        Some(RESOLVED_DYNAMIC_VALUE_METHODS.methods())
    }
}

/// The resolved form of a `DynamicValue` containing the actual providers produced by a dynamic action.
///
/// `ResolvedDynamicValue` is automatically created when a `DynamicValue` is passed to a dynamic
/// action's implementation function via `dynattrs.dynamic_value()`. It provides access to the
/// providers that were produced by the originating dynamic action through its `providers` attribute.
///
/// See [`DynamicValue`](../DynamicValue) for more information
/// ```
#[starlark_module]
fn resolved_dynamic_value_methods(method: &mut MethodsBuilder) {
    /// Get providers from the resolved dynamic value.
    ///
    /// # Example
    ///
    /// ```python
    /// def _impl(actions: AnalysisActions, v: ResolvedDynamicValue, out: OutputArtifact):
    ///     # Access providers
    ///     default_info = v.providers[DefaultInfo]
    ///     custom_info = v.providers[MyInfo]
    /// ```
    #[starlark(attribute)]
    fn providers<'v>(
        this: ValueTyped<'v, StarlarkResolvedDynamicValue<'v>>,
    ) -> starlark::Result<FrozenValueTyped<'v, ProviderCollection<'v>>> {
        Ok(this.as_ref().value)
    }
}

#[starlark_module]
#[starlark_types(
    StarlarkResolvedDynamicValue<'_> as ResolvedDynamicValue
)]
pub fn register_resolved_dynamic_value(globals: &mut GlobalsBuilder) {}
