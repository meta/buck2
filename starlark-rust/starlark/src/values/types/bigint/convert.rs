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

use std::convert::Infallible;

use num_bigint::BigInt;

use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::ValueOf;
use crate::values::types::int::int_or_big::StarlarkInt;
use crate::values::types::int::int_or_big::StarlarkIntRef;
use crate::values::types::int::view::IntView;

impl<'v> AllocValue<'v> for u32 {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for u32 {
    type Type = IntView<'fv>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl<'v> AllocValue<'v> for u64 {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for u64 {
    type Type = IntView<'fv>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl<'v> AllocValue<'v> for i64 {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for i64 {
    type Type = IntView<'fv>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl<'v> AllocValue<'v> for usize {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for usize {
    type Type = IntView<'fv>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl<'v> AllocValue<'v> for isize {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for isize {
    type Type = IntView<'fv>;

    #[inline]
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl<'v> AllocValue<'v> for BigInt {
    type Type = IntView<'v>;

    #[inline]
    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        StarlarkInt::from(self).alloc_value(heap)
    }
}

impl<'fv> AllocFrozenValue<'fv> for BigInt {
    type Type = IntView<'fv>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        StarlarkInt::from(self).alloc_frozen_value(heap)
    }
}

impl<'v> UnpackValue<'v> for u32 {
    type Type = IntView<'v>;
    type Error = crate::Error;

    #[inline]
    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> crate::Result<Self> {
        int.to_integer()
    }
}

impl<'v> UnpackValue<'v> for u64 {
    type Type = IntView<'v>;
    type Error = crate::Error;

    #[inline]
    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> crate::Result<Self> {
        int.to_integer()
    }
}

impl<'v> UnpackValue<'v> for i64 {
    type Type = IntView<'v>;
    type Error = crate::Error;

    #[inline]
    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> crate::Result<Self> {
        int.to_integer()
    }
}

impl<'v> UnpackValue<'v> for usize {
    type Type = IntView<'v>;
    type Error = crate::Error;

    #[inline]
    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> crate::Result<Self> {
        int.to_integer()
    }
}

impl<'v> UnpackValue<'v> for isize {
    type Type = IntView<'v>;
    type Error = crate::Error;

    #[inline]
    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> crate::Result<Self> {
        int.to_integer()
    }
}

impl<'v> UnpackValue<'v> for BigInt {
    type Type = IntView<'v>;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, IntView<'v>>, int: IntView<'v>) -> Result<Self, Infallible> {
        Ok(match int.as_int_ref() {
            StarlarkIntRef::Small(x) => BigInt::from(x.to_i32()),
            StarlarkIntRef::Big(x) => x.get().to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use starlark_derive::starlark_module;

    use crate as starlark;
    use crate::assert::Assert;
    use crate::environment::GlobalsBuilder;
    use crate::values::none::NoneType;

    #[test]
    fn test_unpack_int_error() {
        #[starlark_module]
        fn module(globals: &mut GlobalsBuilder) {
            fn takes_i32(#[starlark(require=pos)] _i: i32) -> starlark::Result<NoneType> {
                Ok(NoneType)
            }

            fn takes_i64(#[starlark(require=pos)] _i: i64) -> starlark::Result<NoneType> {
                Ok(NoneType)
            }
        }

        let mut a = Assert::new();
        a.globals_add(module);
        a.fails(
            "takes_i32(1 << 100)",
            &[
                "Integer value is too big to fit in i32: 1267650600228229401496703205376",
                "Error unpacking value for parameter `_i`",
            ],
        );
        a.fails(
            "takes_i64(1 << 100)",
            &[
                "Integer value is too big to fit in i64: 1267650600228229401496703205376",
                "Error unpacking value for parameter `_i`",
            ],
        );
    }
}
