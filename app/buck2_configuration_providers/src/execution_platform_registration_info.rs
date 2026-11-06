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
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::starlark_module;
use starlark::values::Freeze;
use starlark::values::StarlarkPagable;
use starlark::values::StringValue;
use starlark::values::Trace;
use starlark::values::Value;
use starlark::values::ValueOf;
use starlark::values::ValueOfUnchecked;
use starlark::values::ValueTyped;
use starlark::values::list::ListRef;
use starlark::values::list::ListType;
use starlark::values::none::NoneOr;

use crate::execution_platform_info::ExecutionPlatformInfo;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Input)]
enum ExecutionPlatformRegistrationTypeError {
    #[error("expected a list of ExecutionPlatformInfo, got `{0}` (type `{1}`)")]
    ExpectedListOfPlatforms(String, String),
    #[error("expected a ExecutionPlatformInfo, got `{0}` (type `{1}`)")]
    NotAPlatform(String, String),
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
    platforms: ValueOfUnchecked<'v, Vec<ExecutionPlatformInfo<'static>>>,
    /// Specifies the behavior when no compatible execution platform is found from the `platforms` list.
    /// Can be one of:
    /// - `None` or `"use_unspecified"`: Proceed with an unspecified execution platform.
    ///   This allows the build to continue without explicitly matching an execution platform.
    /// - `"error"`: Fail the build with an error message indicating no compatible platform was found.
    /// - An `ExecutionPlatformInfo`: Use this specific platform as a fallback when no other
    ///   platform from the `platforms` list matches.
    // TODO(nga): specify type more precisely.
    fallback: ValueOfUnchecked<'v, Value<'static>>,
    /// Optional marker constraint that identifies platforms as execution platforms.
    /// If set, every execution platform in `platforms` will be marked with this constraint,
    /// allowing to distinguish execution platforms from target platforms.
    exec_marker_constraint: ValueOfUnchecked<'v, Option<String>>,
}

impl<'v> ExecutionPlatformRegistrationInfo<'v> {
    // TODO(cjhopman): Validity could be checked when constructed rather than only when used.
    pub fn platforms(&self) -> buck2_error::Result<Vec<ValueTyped<'v, ExecutionPlatformInfo<'v>>>> {
        ListRef::from_value(self.platforms.get())
            .ok_or_else(|| {
                ExecutionPlatformRegistrationTypeError::ExpectedListOfPlatforms(
                    self.platforms.get().to_repr(),
                    self.platforms.get().get_type().to_owned(),
                )
            })?
            .iter()
            .map(|v| {
                ValueTyped::new(v).ok_or_else(|| {
                    ExecutionPlatformRegistrationTypeError::NotAPlatform(
                        v.to_repr(),
                        v.get_type().to_owned(),
                    )
                    .into()
                })
            })
            .collect::<buck2_error::Result<_>>()
    }

    pub fn fallback(&self) -> buck2_error::Result<ExecutionPlatformFallback> {
        if self.fallback.get().is_none() {
            return Ok(ExecutionPlatformFallback::UseUnspecifiedExec);
        }
        let fallback = self.fallback.get();
        if let Some(v) = ExecutionPlatformInfo::from_value(fallback) {
            return Ok(ExecutionPlatformFallback::Platform(
                v.to_execution_platform()?,
            ));
        }

        match fallback.unpack_str() {
            Some("error") => Ok(ExecutionPlatformFallback::Error),
            Some("use_unspecified") => Ok(ExecutionPlatformFallback::UseUnspecifiedExec),
            _ => Err(ExecutionPlatformRegistrationTypeError::InvalidFallback(
                fallback.to_repr(),
                fallback.get_type().to_owned(),
            )
            .into()),
        }
    }

    pub fn exec_marker_constraint(&self) -> Option<&str> {
        let value = self.exec_marker_constraint.get();
        if value.is_none() {
            None
        } else {
            value.unpack_str()
        }
    }
}

#[starlark_module]
fn info_creator(globals: &mut GlobalsBuilder) {
    fn ExecutionPlatformRegistrationInfo<'v>(
        #[starlark(require = named)] platforms: ValueOf<
            'v,
            ListType<ValueTyped<'v, ExecutionPlatformInfo<'v>>>,
        >,
        #[starlark(require = named, default = NoneOr::None)] fallback: NoneOr<Value<'v>>,
        #[starlark(require = named, default = NoneOr::None)] exec_marker_constraint: NoneOr<
            StringValue<'v>,
        >,
    ) -> starlark::Result<ExecutionPlatformRegistrationInfo<'v>> {
        Ok(ExecutionPlatformRegistrationInfo {
            platforms: ValueOfUnchecked::new(platforms.to_value()),
            fallback: ValueOfUnchecked::new(match fallback {
                NoneOr::None => Value::new_none(),
                NoneOr::Other(v) => v,
            }),
            exec_marker_constraint: ValueOfUnchecked::new(match exec_marker_constraint {
                NoneOr::None => Value::new_none(),
                NoneOr::Other(v) => v.to_value(),
            }),
        })
    }
}
