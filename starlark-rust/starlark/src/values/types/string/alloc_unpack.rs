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

use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::StringValue;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::alloc_value::AllocFrozenStringValue;
use crate::values::alloc_value::AllocStringValue;
use crate::values::string::str_type::StarlarkStr;
use crate::values::type_repr::StarlarkTypeRepr;

impl<'fv> AllocFrozenValue<'fv> for String {
    type Type = StarlarkStr;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        self.alloc_frozen_string_value(heap).into()
    }
}

impl<'fv> AllocFrozenStringValue<'fv> for String {
    fn alloc_frozen_string_value(self, heap: FrozenHeap<'fv>) -> StringValue<'fv> {
        heap.alloc_str(self.as_str())
    }
}

impl<'fv, 'a> AllocFrozenValue<'fv> for &'a str {
    type Type = StarlarkStr;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        self.alloc_frozen_string_value(heap).into()
    }
}

impl<'fv, 'a> AllocFrozenStringValue<'fv> for &'a str {
    fn alloc_frozen_string_value(self, heap: FrozenHeap<'fv>) -> StringValue<'fv> {
        heap.alloc_str(self)
    }
}

impl<'v> AllocValue<'v> for String {
    type Type = StarlarkStr;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self.alloc_string_value(heap).into()
    }
}

impl<'v> AllocStringValue<'v> for String {
    fn alloc_string_value(self, heap: Heap<'v>) -> StringValue<'v> {
        heap.alloc_str(self.as_str())
    }
}

impl StarlarkTypeRepr for char {
    type Canonical = <String as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        String::starlark_type_repr()
    }
}

impl<'v> AllocValue<'v> for char {
    type Type = StarlarkStr;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self.alloc_string_value(heap).into()
    }
}

impl<'v> AllocStringValue<'v> for char {
    fn alloc_string_value(self, heap: Heap<'v>) -> StringValue<'v> {
        heap.alloc_char(self)
    }
}

impl StarlarkTypeRepr for &'_ String {
    type Canonical = <String as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        String::starlark_type_repr()
    }
}

impl<'v> AllocValue<'v> for &'_ String {
    type Type = StarlarkStr;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self.alloc_string_value(heap).into()
    }
}

impl<'v> AllocStringValue<'v> for &'_ String {
    fn alloc_string_value(self, heap: Heap<'v>) -> StringValue<'v> {
        heap.alloc_str(self.as_str())
    }
}

impl<'v> AllocValue<'v> for &'_ str {
    type Type = StarlarkStr;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        self.alloc_string_value(heap).into()
    }
}

impl<'v> AllocStringValue<'v> for &'_ str {
    fn alloc_string_value(self, heap: Heap<'v>) -> StringValue<'v> {
        heap.alloc_str(self)
    }
}

impl<'v> UnpackValue<'v> for &'v str {
    type Type = &'v str;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, &'v str>, get: &'v str) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl<'v> UnpackValue<'v> for String {
    type Type = String;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, String>, get: String) -> Result<Self, Infallible> {
        Ok(get)
    }
}
