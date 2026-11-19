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

//! Tests for `const` items in a `#[starlark_module]`.

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_module;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::assert::Assert;
use crate::environment::GlobalsBuilder;
use crate::values::StarlarkValue;

#[derive(
    Debug,
    derive_more::Display,
    Allocative,
    ProvidesStaticType,
    NoSerialize,
    StarlarkPagable
)]
#[display("marker")]
struct Marker;

#[starlark_value(type = "marker")]
impl<'v> StarlarkValue<'v> for Marker {}

#[starlark_module]
fn constants(globals: &mut GlobalsBuilder) {
    const ANSWER: i32 = 42;

    const MARKER: Marker = Marker;
}

#[test]
fn test_constants() {
    let mut a = Assert::new();
    a.globals_add(constants);
    a.eq("ANSWER", "42");
    a.eq("repr(MARKER)", "'marker'");
    a.eq("type(MARKER)", "'marker'");
}
