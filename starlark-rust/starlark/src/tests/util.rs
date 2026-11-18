/*
 * Copyright 2018 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

#![cfg(test)]

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::Trace;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::values::Freeze;
use crate::values::FreezeResult;
use crate::values::Freezer;
use crate::values::StarlarkValue;
use crate::values::Value;

#[derive(
    Trace,
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("TestComplexValue<{}>", _0)]
pub(crate) struct TestComplexValue<'v>(pub(crate) Value<'v>);

#[starlark_value(type = "TestComplexValue", frozen_vtable, alloc = complex)]
impl<'v> StarlarkValue<'v> for TestComplexValue<'v> {}

impl<'v> Freeze<'v> for TestComplexValue<'v> {
    type Frozen<'fv> = TestComplexValue<'fv>;

    fn freeze<'fv>(self, freezer: &Freezer<'v, 'fv>) -> FreezeResult<Self::Frozen<'fv>> {
        Ok(TestComplexValue(freezer.freeze(self.0)?))
    }
}

/// There's no anyhow API to print error without rust backtrace
/// ([issue](https://github.com/dtolnay/anyhow/issues/300)).
pub(crate) fn trim_rust_backtrace(error: &str) -> &str {
    match error.find("\nStack backtrace:") {
        Some(pos) => error[..pos].trim_end(),
        None => error.trim_end(),
    }
}
