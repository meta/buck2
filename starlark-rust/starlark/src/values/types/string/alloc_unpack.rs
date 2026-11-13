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

//! Implementations of alloc and unpack traits for string.

use std::convert::Infallible;

use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::string::str_type::StarlarkStr;

impl<'fv> AllocFrozenValue<'fv> for String {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        heap.alloc_str(self.as_str())
    }
}

impl<'fv, 'a> AllocFrozenValue<'fv> for &'a str {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        heap.alloc_str(self)
    }
}

impl<'v> AllocValue<'v> for String {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        heap.alloc_str(self.as_str())
    }
}

impl<'v> AllocValue<'v> for char {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        heap.alloc_char(self)
    }
}

impl<'v> AllocValue<'v> for &'_ String {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        heap.alloc_str(self.as_str())
    }
}

impl<'v> AllocValue<'v> for &'_ str {
    type Type = StarlarkStr;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        heap.alloc_str(self)
    }
}

impl<'v> UnpackValue<'v> for &'v str {
    type Type = StarlarkStr;
    type Error = Infallible;

    #[inline]
    fn unpack(
        _value: ValueOf<'v, StarlarkStr>,
        get: ValueTyped<'v, StarlarkStr>,
    ) -> Result<Self, Infallible> {
        Ok(get.as_ref().as_str())
    }
}
