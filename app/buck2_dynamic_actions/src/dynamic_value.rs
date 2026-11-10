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
use buck2_build_api::dynamic_value::DynamicValue;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::Value;
use starlark::values::ValueTyped;
use starlark::values::starlark_value;
use starlark_map::StarlarkHasher;

#[derive(
    Debug,
    ProvidesStaticType,
    derive_more::Display,
    Allocative,
    NoSerialize,
    StarlarkPagable
)]
#[display("DynamicValue<{}>", self.dynamic_value)]
pub struct StarlarkDynamicValue {
    #[starlark_pagable(pagable)]
    pub dynamic_value: DynamicValue,
}

starlark::methods_static!(DYNAMIC_VALUE_METHODS = dynamic_value_methods);

#[starlark_value(type = "DynamicValue")]
impl<'v> StarlarkValue<'v> for StarlarkDynamicValue {
    fn write_hash(&self, hasher: &mut StarlarkHasher) -> starlark::Result<()> {
        self.dynamic_value.hash(hasher);
        Ok(())
    }

    fn equals(&self, other: Value<'v>) -> starlark::Result<bool> {
        let Some(other) = ValueTyped::<StarlarkDynamicValue>::new(other) else {
            return Ok(false);
        };
        Ok(self.dynamic_value == other.dynamic_value)
    }

    // used for docs of `DynamicValue`
    fn get_methods() -> Option<&'static Methods> {
        Some(DYNAMIC_VALUE_METHODS.methods())
    }
}

#[starlark_module]
#[starlark_types(StarlarkDynamicValue as DynamicValue)]
pub fn register_dynamic_value(globals: &mut GlobalsBuilder) {}

/// A value produced by a dynamic action that can be consumed by other dynamic actions.
///
/// `DynamicValue` is returned by `ctx.actions.dynamic_output_new()` and represents the
/// providers that will be produced by executing a dynamic action. It can be passed as
/// input to other dynamic actions via `dynattrs.dynamic_value()`, enabling chaining of
/// dynamic actions where one action's output providers are consumed by another.
///
/// When a `DynamicValue` is passed to a dynamic action's implementation function, it
/// is automatically resolved to a `ResolvedDynamicValue` containing the actual providers.
///
/// # Usage
///
/// ```python
/// # First dynamic action produces a DynamicValue
/// def _produce_impl(actions: AnalysisActions):
///     return [DefaultInfo(), MyInfo(value = 42)]
///
/// _produce = dynamic_actions(impl = _produce_impl, attrs = {})
///
/// # Get the DynamicValue
/// dynamic_val = ctx.actions.dynamic_output_new(_produce())
///
/// # Second dynamic action consumes the DynamicValue
/// def _consume_impl(actions: AnalysisActions, v: ResolvedDynamicValue, out: OutputArtifact):
///     # Access the providers from the resolved value
///     value = v.providers[MyInfo].value
///     actions.write(out, str(value))
///     return [DefaultInfo()]
///
/// _consume = dynamic_actions(
///     impl = _consume_impl,
///     attrs = {
///         "v": dynattrs.dynamic_value(),
///         "out": dynattrs.output(),
///     },
/// )
///
/// # Pass the DynamicValue to the consuming action
/// ctx.actions.dynamic_output_new(_consume(v = dynamic_val, out = output.as_output()))
/// ```
#[starlark_module]
fn dynamic_value_methods(builder: &mut MethodsBuilder) {}
