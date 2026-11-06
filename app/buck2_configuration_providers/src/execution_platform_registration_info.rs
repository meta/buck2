/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;

use allocative::Allocative;
use buck2_build_api_derive::internal_provider;
use buck2_core::execution_types::execution_platforms::ExecutionPlatformFallback;
use either::Either;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::ValueOf;
use starlark::values::ValueTyped;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;

use crate::execution_platform_info::ExecutionPlatformInfo;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ExecutionPlatformRegistrationTypeError {
    #[error(
        "expected an ExecutionPlatformInfo or one of the strings \"error\" or \"use_unspecified\", got `{0}` (type `{1}`)"
    )]
    InvalidFallback(String, String),
}

/// Provider that gives the list of all execution platforms available for this build.
#[internal_provider(info_creator)]
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
pub struct ExecutionPlatformRegistrationInfo<'v> {
    /// The list of execution platforms that are available for the build.
    platforms: ValueOf<'v, ListType<ValueTyped<'v, ExecutionPlatformInfo<'v>>>>,
    /// Specifies the behavior when no compatible execution platform is found from the `platforms` list.
    /// Can be one of:
    /// - `None` or `"use_unspecified"`: Proceed with an unspecified execution platform.
    ///   This allows the build to continue without explicitly matching an execution platform.
    /// - `"error"`: Fail the build with an error message indicating no compatible platform was found.
    /// - An `ExecutionPlatformInfo`: Use this specific platform as a fallback when no other
    ///   platform from the `platforms` list matches.
    fallback:
        Option<ValueOf<'v, Either<StringValue<'v>, ValueTyped<'v, ExecutionPlatformInfo<'v>>>>>,
    /// Optional marker constraint that identifies platforms as execution platforms.
    /// If set, every execution platform in `platforms` will be marked with this constraint,
    /// allowing to distinguish execution platforms from target platforms.
    exec_marker_constraint: Option<StringValue<'v>>,
}

type Fallback<'v> = ValueOf<'v, Either<StringValue<'v>, ValueTyped<'v, ExecutionPlatformInfo<'v>>>>;

impl<'v> ExecutionPlatformRegistrationInfo<'v> {
    pub fn platforms(&self) -> buck2_error::Result<Vec<ValueTyped<'v, ExecutionPlatformInfo<'v>>>> {
        self.platforms.iter().map(|p| Ok(p?.unpack())).collect()
    }

    pub fn fallback(&self) -> buck2_error::Result<ExecutionPlatformFallback> {
        match self.fallback.map(|f| f.unpack()) {
            None => Ok(ExecutionPlatformFallback::UseUnspecifiedExec),
            Some(Either::Left(word)) => fallback_from_str(word),
            Some(Either::Right(platform)) => Ok(ExecutionPlatformFallback::Platform(
                platform.to_execution_platform()?,
            )),
        }
    }

    pub fn exec_marker_constraint(&self) -> Option<&'v str> {
        self.exec_marker_constraint.map(|s| s.as_str())
    }
}

fn fallback_from_str(word: StringValue) -> buck2_error::Result<ExecutionPlatformFallback> {
    match word.as_str() {
        "error" => Ok(ExecutionPlatformFallback::Error),
        "use_unspecified" => Ok(ExecutionPlatformFallback::UseUnspecifiedExec),
        _ => Err(ExecutionPlatformRegistrationTypeError::InvalidFallback(
            word.to_value().to_repr(),
            word.to_value().get_type().to_owned(),
        )
        .into()),
    }
}

#[starlark_module]
fn info_creator(globals: &mut GlobalsBuilder) {
    fn ExecutionPlatformRegistrationInfo<'v>(
        #[starlark(require = named)] platforms: ValueOf<
            'v,
            ListType<ValueTyped<'v, ExecutionPlatformInfo<'v>>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] fallback: NoneOr<Fallback<'v>>,
        #[starlark(require = named, default = NoneOr::None)] exec_marker_constraint: NoneOr<
            StringValue<'v>,
        >,
    ) -> starlark::Result<ExecutionPlatformRegistrationInfo<'v>> {
        let fallback = fallback.into_option();
        if let Some(Either::Left(word)) = fallback.map(|f| f.unpack()) {
            fallback_from_str(word)?;
        }
        Ok(ExecutionPlatformRegistrationInfo {
            platforms,
            fallback,
            exec_marker_constraint: exec_marker_constraint.into_option(),
        })
    }
}
