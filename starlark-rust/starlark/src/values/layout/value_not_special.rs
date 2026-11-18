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

use dupe::Dupe;
use starlark_derive::StarlarkPagable;

use crate as starlark;
use crate::values::Value;
use crate::values::layout::vtable::AValueDynFull;
use crate::values::stack_guard;

/// A frozen `Value` which is not `i32` or `str`, at the brand of the heap it lives in.
#[derive(Copy, Clone, Dupe, Debug, derive_more::Display, StarlarkPagable)]
pub(crate) struct ValueNotSpecial<'v>(Value<'v>);

impl<'v> ValueNotSpecial<'v> {
    #[inline]
    pub(crate) fn new(value: Value<'v>) -> Option<ValueNotSpecial<'v>> {
        if value.is_str() || value.unpack_inline_int().is_some() || !value.is_frozen() {
            None
        } else {
            Some(ValueNotSpecial(value))
        }
    }

    #[inline]
    pub(crate) fn to_value(self) -> Value<'v> {
        self.0
    }

    #[inline]
    fn get_ref(self) -> AValueDynFull<'v> {
        // SAFETY: we checked in constructor that it is a frozen value which is not a str or i32,
        // and the vtable is read from the value itself.
        unsafe { AValueDynFull::new(self.0.0.value_header_ptr_unchecked().unpack(), self.0) }
    }

    #[inline]
    pub(crate) fn equals(self, other: Value<'v>) -> crate::Result<bool> {
        if self.to_value().ptr_eq(other) {
            Ok(true)
        } else {
            // Condition and then branch are cheap, but else branch is not.
            // Split it so the compiler could inline this function
            // without hitting the inlining limit.
            self.equals_not_ptr_eq(other)
        }
    }

    #[inline]
    fn equals_not_ptr_eq(self, other: Value<'v>) -> crate::Result<bool> {
        let _guard = stack_guard::stack_guard()?;
        self.get_ref().equals(other)
    }
}
