/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Display;
use std::fmt::Formatter;

use allocative::Allocative;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use starlark::any::ProvidesStaticType;
use starlark::starlark_simple_value;
use starlark::values::Freeze;
use starlark::values::NoSerialize;
use starlark::values::StarlarkPagable;
use starlark::values::StarlarkValue;
use starlark::values::starlark_value;

#[derive(
    ProvidesStaticType,
    Debug,
    NoSerialize,
    Allocative,
    Freeze,
    StarlarkPagable
)]
pub struct StarlarkExecutionPlatformResolution(
    #[starlark_pagable(pagable)]
    #[freeze(identity)]
    pub ExecutionPlatformResolution,
);

starlark_simple_value!(StarlarkExecutionPlatformResolution);

#[starlark_value(type = "ExecutionPlatformResolution")]
impl<'v> StarlarkValue<'v> for StarlarkExecutionPlatformResolution {}

impl Display for StarlarkExecutionPlatformResolution {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self.0.platform() {
            Ok(p) => write!(f, "{}", p.id()),
            Err(e) => write!(f, "{e}"),
        }
    }
}
