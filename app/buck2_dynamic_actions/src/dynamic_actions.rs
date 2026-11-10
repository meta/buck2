/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cell::RefCell;

use allocative::Allocative;
use starlark::any::ProvidesStaticType;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::values::FrozenValueTyped;
use starlark::values::NoSerialize;
use starlark::values::StarlarkValue;
use starlark::values::Trace;
use starlark::values::starlark_value;

use crate::attrs::DynamicAttrValues;
use crate::dynamic_actions_callable::FrozenStarlarkDynamicActionsCallable;

#[derive(Debug, Trace, Allocative)]
pub struct StarlarkDynamicActionsData<'v> {
    pub callable: FrozenValueTyped<'v, FrozenStarlarkDynamicActionsCallable<'v>>,
    pub attr_values: DynamicAttrValues<'v>,
}

#[derive(
    Debug,
    NoSerialize,
    ProvidesStaticType,
    derive_more::Display,
    Trace,
    Allocative
)]
#[display("DynamicActions<...>")]
pub struct StarlarkDynamicActions<'v> {
    pub data: RefCell<Option<StarlarkDynamicActionsData<'v>>>,
}

starlark::methods_static!(DYNAMIC_ACTIONS_METHODS = dynamic_actions_methods);

// TODO(nero): the type name is not aligan with the registered type `DynamicActions`, fix it.
#[starlark_value(type = "DynamicAction", no_freeze)]
impl<'v> StarlarkValue<'v> for StarlarkDynamicActions<'v> {
    // Used to add type documentation to the generated documentation
    fn get_methods() -> Option<&'static Methods> {
        Some(DYNAMIC_ACTIONS_METHODS.methods())
    }
}

/// A dynamic action ready to be passed to `ctx.actions.dynamic_output_new()`.
///
/// `DynamicActions` represents a deferred computation that will read artifact contents and produce
/// outputs based on those contents. It is created by calling a `DynamicActionsCallable` (returned
/// by `dynamic_actions()`) with concrete artifact values.
///
/// # Usage
///
/// ```python
/// # `my_dynamic` is a `DynamicActionsCallable` created with `dynamic_actions()`
/// dynamic_action = my_dynamic(
///     config = config_file,
///     out = output.as_output(),
/// )
///
/// # Declare the dynamic action in the build's action graph
/// ctx.actions.dynamic_output_new(dynamic_action)
/// ```
///
/// See `dynamic_actions()` and `DynamicActionsCallable` for details on creating the factory function.
#[starlark_module]
fn dynamic_actions_methods(builder: &mut MethodsBuilder) {}
