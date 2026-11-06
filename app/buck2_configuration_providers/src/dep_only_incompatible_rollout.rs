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
use buck2_build_api_derive::internal_provider;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::list::ListType;

/// A provider that is used to signal the targets that we want to soft error on
/// within the `DepOnlyIncompatibleInfo` provider.
#[internal_provider(dep_only_incompatible_custom_soft_error_creator)]
#[derive(
    Clone,
    Debug,
    Trace,
    Freeze,
    ProvidesStaticType,
    Allocative,
    StarlarkPagable
)]
#[repr(C)]
pub struct DepOnlyIncompatibleRollout<'v> {
    /// A list of target patterns (minus exclusions) that we want to soft error on.
    pub target_patterns: ValueOf<'v, ListType<StringValue<'v>>>,
    /// A list of target patterns that we want to exclude from the soft error.
    pub exclusions: ValueOf<'v, ListType<StringValue<'v>>>,
}

#[starlark_module]
fn dep_only_incompatible_custom_soft_error_creator(globals: &mut GlobalsBuilder) {
    #[starlark(as_type = DepOnlyIncompatibleRollout<'static>)]
    fn DepOnlyIncompatibleRollout<'v>(
        #[starlark(require = named)] target_patterns: ValueOf<'v, ListType<StringValue<'v>>>,
        #[starlark(require = named)] exclusions: ValueOf<'v, ListType<StringValue<'v>>>,
    ) -> starlark::Result<DepOnlyIncompatibleRollout<'v>> {
        Ok(DepOnlyIncompatibleRollout {
            target_patterns,
            exclusions,
        })
    }
}
