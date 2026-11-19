/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Instant;

use allocative::Allocative;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkValue;
use starlark::values::starlark_value;

/// Starlark object for Instant.
#[derive(
    Clone,
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    starlark::StarlarkPagableUnsupported
)]
#[display("{:?}", _0)]
pub(crate) struct StarlarkInstant(pub(crate) Instant);

/// Instant methods, to aid in debugging/timing individual pieces of the bxl script.
#[starlark_module]
fn starlark_instant_methods(builder: &mut MethodsBuilder) {
    /// Elapsed time in secs as a float
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_elapsed_secs(ctx):
    ///     now = now()
    ///     time_a = now.elapsed_secs()
    ///     # do something that takes a long time
    ///     time_b = now.elapsed_secs()
    ///
    ///     ctx.output.print(time_a)
    ///     ctx.output.print(time_b)
    /// ```
    fn elapsed_secs(this: &StarlarkInstant) -> starlark::Result<f64> {
        let secs = (Instant::now() - this.0).as_secs() as f64;

        Ok(secs)
    }

    /// Elapsed time in millis as a float
    ///
    /// Sample usage:
    /// ```python
    /// def _impl_elapsed_millis(ctx):
    ///     now = now()
    ///     time_a = now.elapsed_millis()
    ///     # do something that takes a long time
    ///     time_b = now.elapsed_millis()
    ///
    ///     ctx.output.print(time_a)
    ///     ctx.output.print(time_b)
    /// ```
    fn elapsed_millis(this: &StarlarkInstant) -> starlark::Result<f64> {
        let millis = (Instant::now() - this.0).as_millis() as f64;

        Ok(millis)
    }
}

starlark_simple_value!(StarlarkInstant);

starlark::methods_static!(STARLARK_INSTANT_METHODS = starlark_instant_methods);

#[starlark_value(type = "bxl.Instant")]
impl<'v> StarlarkValue<'v> for StarlarkInstant {
    fn get_methods() -> Option<&'static Methods> {
        Some(STARLARK_INSTANT_METHODS.methods())
    }
}
