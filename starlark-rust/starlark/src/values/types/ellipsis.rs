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

use allocative::Allocative;
use starlark_derive::NoSerialize;
use starlark_derive::ProvidesStaticType;
use starlark_derive::StarlarkPagable;
use starlark_derive::starlark_value;

use crate as starlark;
use crate::static_starlark_value;
use crate::values::AllocFrozenValue;
use crate::values::FrozenHeap;
use crate::values::StarlarkValue;
use crate::values::Value;
use crate::values::ValueOf;

#[derive(
    Allocative,
    NoSerialize,
    Debug,
    derive_more::Display,
    ProvidesStaticType,
    StarlarkPagable
)]
#[display("Ellipsis")]
pub(crate) struct Ellipsis;

static_starlark_value!(pub(crate) VALUE_ELLIPSIS: Ellipsis = Ellipsis);

#[starlark_value(type = "ellipsis")]
impl<'v> StarlarkValue<'v> for Ellipsis {}

impl Ellipsis {
    pub(crate) fn new_value<'v>() -> Value<'v> {
        VALUE_ELLIPSIS.at().to_value()
    }
}

impl<'fv> AllocFrozenValue<'fv> for Ellipsis {
    type Type = Self;

    fn alloc_frozen_value(self, _heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        VALUE_ELLIPSIS.at().into()
    }
}

#[cfg(test)]
mod tests {
    use crate::assert;

    #[test]
    fn test_ellipsis() {
        assert::pass("...");
    }
}
