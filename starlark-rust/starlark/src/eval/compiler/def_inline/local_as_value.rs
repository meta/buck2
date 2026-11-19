/*
 * Copyright 2019 The Starlark in Rust Authors.
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

//! Local slot id as value.
//!
//! To be able to propagate the local slot number through parameter binding machinery.

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::eval::runtime::slots::LocalSlotId;
use crate::starlark_simple_value;
use crate::values::ProvidesStaticType;
use crate::values::StarlarkValue;

/// Local slot id as a value. This object is only used during compilation, allocated on the
/// compiler's frozen heap (see `OptCtx::local_as_values`), and never appears in the executed
/// program.
#[derive(
    derive_more::Display,
    Debug,
    ProvidesStaticType,
    NoSerialize,
    Allocative,
    StarlarkPagable
)]
#[display("{:?}", self)]
pub(crate) struct LocalAsValue {
    #[allocative(skip)]
    pub(crate) local: LocalSlotId,
}

#[starlark_value(type = "LocalAsValue")]
impl<'v> StarlarkValue<'v> for LocalAsValue {}

starlark_simple_value!(LocalAsValue);
