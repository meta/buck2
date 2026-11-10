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
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::values::Heap;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::ValueOf;
use starlark::values::starlark_value;

/// Object describing which local resources are needed for a given test rule.
#[derive(
    Debug,
    Display,
    NoSerialize,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[display(
    "RequiredTestLocalResource(name: {}, listing: {}, execution: {})",
    self.name,
    self.listing,
    self.execution
)]
pub struct StarlarkRequiredTestLocalResource {
    /// Local resource type
    pub name: String,
    /// Is it needed for test listing
    pub listing: bool,
    /// Is it needed for test execution
    pub execution: bool,
}

starlark_simple_value!(StarlarkRequiredTestLocalResource);

starlark::methods_static!(
    REQUIRED_TEST_LOCAL_RESOURCE_METHODS = required_test_local_resource_methods
);

#[starlark_value(type = "RequiredTestLocalResource")]
impl<'v> StarlarkValue<'v> for StarlarkRequiredTestLocalResource {
    fn get_methods() -> Option<&'static Methods> {
        Some(REQUIRED_TEST_LOCAL_RESOURCE_METHODS.methods())
    }
}

#[starlark_module]
pub fn register_required_test_local_resource(builder: &mut GlobalsBuilder) {
    #[starlark(as_type = StarlarkRequiredTestLocalResource)]
    fn RequiredTestLocalResource<'v>(
        #[starlark(require = pos)] name: String,
        #[starlark(require = named, default = true)] listing: bool,
        #[starlark(require = named, default = true)] execution: bool,
        heap: Heap<'v>,
    ) -> starlark::Result<ValueOf<'v, StarlarkRequiredTestLocalResource>> {
        if !(listing || execution) {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::StarlarkError,
                "`RequiredTestLocalResource` should not be disabled for both listing and execution stages",
            ).into());
        }
        Ok(ValueOf::alloc(
            heap,
            StarlarkRequiredTestLocalResource {
                name,
                listing,
                execution,
            },
        ))
    }
}

#[starlark_module]
fn required_test_local_resource_methods(builder: &mut MethodsBuilder) {
    #[starlark(attribute)]
    /// Local resource type
    fn name<'v>(this: &'v StarlarkRequiredTestLocalResource) -> starlark::Result<&'v str> {
        Ok(&this.name)
    }

    #[starlark(attribute)]
    /// Is this resource type needed for test listing?
    fn listing<'v>(this: &'v StarlarkRequiredTestLocalResource) -> starlark::Result<bool> {
        Ok(this.listing)
    }

    #[starlark(attribute)]
    /// Is this resource type needed for test execution?
    fn execution<'v>(this: &'v StarlarkRequiredTestLocalResource) -> starlark::Result<bool> {
        Ok(this.execution)
    }
}
