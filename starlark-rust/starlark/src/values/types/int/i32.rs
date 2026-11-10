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

use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::int::pointer_i32::PointerI32;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::int::int_or_big::StarlarkInt;
use crate::values::types::int::view::IntView;

impl<'v> AllocValue<'v> for i32 {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}
impl<'fv> AllocFrozenValue<'fv> for i32 {
    type Type = IntView<'fv>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl StarlarkTypeRepr for i32 {
    type Canonical = <StarlarkInt as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        PointerI32::starlark_type_repr()
    }
}

impl<'v> UnpackValue<'v> for i32 {
    type Type = IntView<'v>;
    type Error = crate::Error;

    #[inline]
    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> crate::Result<Self> {
        int.to_integer()
    }
}
