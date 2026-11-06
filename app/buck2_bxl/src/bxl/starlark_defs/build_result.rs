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
use buck2_build_api::bxl::build_result::BxlBuildResult;
use starlark::environment::Methods;
use starlark::environment::MethodsBuilder;
use starlark::starlark_module;
use starlark::starlark_simple_value;
use starlark::values::NoSerialize;
use starlark::values::ProvidesStaticType;
use starlark::values::StarlarkValue;
use starlark::values::ValueTyped;
use starlark::values::none::NoneOr;
use starlark::values::starlark_value;

use crate::bxl::starlark_defs::context::build::StarlarkFailedArtifactIterable;
use crate::bxl::starlark_defs::context::build::StarlarkProvidersArtifactIterable;

/// Starlark object for `StarlarkBuildResult` (which is not Starlark value).
#[derive(
    Clone,
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    starlark::StarlarkPagableUnsupported
)]
pub(crate) struct StarlarkBxlBuildResult(pub(crate) BxlBuildResult);

/// The result of building in bxl.
#[starlark_module]
fn starlark_build_result_methods(builder: &mut MethodsBuilder) {
    /// Returns an optional iterable of artifacts that was successfully built.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl(ctx):
    ///     outputs = {}
    ///     for target, value in ctx.build(ctx.cli_args.target).items():
    ///         ctx.output.print(value.artifacts())
    /// ```
    fn artifacts<'v>(
        this: ValueTyped<'v, StarlarkBxlBuildResult>,
    ) -> starlark::Result<NoneOr<StarlarkProvidersArtifactIterable<'v>>> {
        match &this.as_ref().0 {
            BxlBuildResult::None => Ok(NoneOr::None),
            BxlBuildResult::Built { .. } => Ok(NoneOr::Other(StarlarkProvidersArtifactIterable(
                this.to_value(),
            ))),
        }
    }

    /// Returns an optional of iterable of artifacts that failed to be built.
    ///
    /// Sample usage:
    /// ```python
    /// def _impl(ctx):
    ///     outputs = {}
    ///     for target, value in ctx.build(ctx.cli_args.target).items():
    ///         ctx.output.print(value.failures())
    /// ```
    fn failures<'v>(
        this: ValueTyped<'v, StarlarkBxlBuildResult>,
    ) -> starlark::Result<NoneOr<StarlarkFailedArtifactIterable<'v>>> {
        match &this.as_ref().0 {
            BxlBuildResult::None => Ok(NoneOr::None),
            BxlBuildResult::Built { .. } => Ok(NoneOr::Other(StarlarkFailedArtifactIterable(
                this.to_value(),
            ))),
        }
    }
}

starlark_simple_value!(StarlarkBxlBuildResult);

starlark::methods_static!(STARLARK_BUILD_RESULT_METHODS = starlark_build_result_methods);

#[starlark_value(type = "bxl.BuildResult")]
impl<'v> StarlarkValue<'v> for StarlarkBxlBuildResult {
    fn get_methods() -> Option<&'static Methods> {
        Some(STARLARK_BUILD_RESULT_METHODS.methods())
    }
}
