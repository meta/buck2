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

//! The behaviour of `int`, written once for its two representations.

use std::cmp::Ordering;
use std::hash::Hasher;

use num_bigint::BigInt;
use starlark_derive::StarlarkRepresentations;
use starlark_derive::starlark_value;
use starlark_map::StarlarkHashValue;
use starlark_map::StarlarkHasher;

use crate as starlark;
use crate::private::Private;
use crate::typing::Ty;
use crate::typing::TyBasic;
use crate::typing::TypingBinOp;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::ValueTyped;
use crate::values::layout::value::IntegerTooBigError;
use crate::values::types::bigint::StarlarkBigInt;
use crate::values::types::int::int_or_big::StarlarkIntRef;
use crate::values::types::int::pointer_i32::INT_TYPE;
use crate::values::types::int::pointer_i32::PointerI32;
use crate::values::types::num::typecheck::NumTy;
use crate::values::types::num::typecheck::typecheck_num_bin_op;
use crate::values::types::num::value::NumRef;

/// An `int` in either of its representations: the type expression for `int`, and what a
/// [`ValueOf`](crate::values::ValueOf) of it is read through. The fixed-width Rust integers
/// are conversions from it, which may fail.
#[derive(Debug, Clone, Copy, StarlarkRepresentations)]
pub enum IntView<'v> {
    /// An integer stored inline in the value pointer. First, so that reading an `int` tests
    /// the common representation first.
    Small(ValueTyped<'v, PointerI32>),
    /// An integer that does not fit the inline representation.
    Big(ValueTyped<'v, StarlarkBigInt>),
}

impl<'v> IntView<'v> {
    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        match self {
            IntView::Big(x) => x.to_value(),
            IntView::Small(x) => x.to_value(),
        }
    }

    /// The integer, by reference.
    #[inline]
    pub fn as_int_ref(self) -> StarlarkIntRef<'v> {
        match self {
            IntView::Big(x) => StarlarkIntRef::Big(x.as_ref()),
            IntView::Small(x) => StarlarkIntRef::Small(x.as_ref().get()),
        }
    }

    #[inline]
    fn num(self) -> NumRef<'v> {
        NumRef::Int(self.as_int_ref())
    }

    /// The integer as a fixed-width Rust integer, or an error if it does not fit.
    pub fn to_integer<I>(self) -> crate::Result<I>
    where
        I: TryFrom<i32> + TryFrom<&'v BigInt>,
    {
        let option = match self.as_int_ref() {
            StarlarkIntRef::Small(x) => I::try_from(x.to_i32()).ok(),
            StarlarkIntRef::Big(x) => x.unpack_integer(),
        };
        option.ok_or_else(|| {
            crate::Error::new_value(IntegerTooBigError {
                integer_type: std::any::type_name::<I>(),
                value: self.to_string(),
            })
        })
    }
}

#[starlark_value(type = INT_TYPE)]
impl<'v> StarlarkType<'v> for IntView<'v> {
    fn to_bool(&self) -> bool {
        self.as_int_ref() != 0
    }

    fn equals(&self, other: Value<'v>) -> crate::Result<bool> {
        Ok(Some(self.num()) == other.unpack_num())
    }

    fn compare(&self, other: Value<'v>) -> crate::Result<Ordering> {
        match other.unpack_num() {
            None => ValueError::unsupported_with(self, "compare", other),
            Some(other) => Ok(self.num().cmp(&other)),
        }
    }

    fn write_hash(&self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        hasher.write_u64(self.num().get_hash_64());
        Ok(())
    }

    fn get_hash(&self, _private: Private) -> crate::Result<StarlarkHashValue> {
        Ok(self.num().get_hash())
    }

    fn plus(&self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(self.to_value())
    }

    fn minus(&self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(heap.alloc(-self.as_int_ref()))
    }

    fn add(&self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(
            (self.num() + other.unpack_num()?)
                .map(|x| heap.alloc(x))
                .map_err(Into::into),
        )
    }

    fn sub(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc((self.num() - other)?)),
            None => ValueError::unsupported_with(self, "-", other),
        }
    }

    fn mul(&self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(
            (self.num() * other.unpack_num()?)
                .map(|x| heap.alloc(x))
                .map_err(Into::into),
        )
    }

    fn div(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc(self.num().div(other)?)),
            None => ValueError::unsupported_with(self, "/", other),
        }
    }

    fn percent(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc(self.num().percent(other)?)),
            None => ValueError::unsupported_with(self, "%", other),
        }
    }

    fn floor_div(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc(self.num().floor_div(other)?)),
            None => ValueError::unsupported_with(self, "//", other),
        }
    }

    fn bit_and(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match (self.as_int_ref(), StarlarkIntRef::unpack(other)) {
            (_, None) => ValueError::unsupported_with(self, "&", other),
            (StarlarkIntRef::Small(a), Some(StarlarkIntRef::Small(b))) => Ok(Value::new_int(a & b)),
            (a, Some(b)) => Ok(heap.alloc(a & b)),
        }
    }

    fn bit_or(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match (self.as_int_ref(), StarlarkIntRef::unpack(other)) {
            (_, None) => ValueError::unsupported_with(self, "|", other),
            (StarlarkIntRef::Small(a), Some(StarlarkIntRef::Small(b))) => Ok(Value::new_int(a | b)),
            (a, Some(b)) => Ok(heap.alloc(a | b)),
        }
    }

    fn bit_xor(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match (self.as_int_ref(), StarlarkIntRef::unpack(other)) {
            (_, None) => ValueError::unsupported_with(self, "^", other),
            (StarlarkIntRef::Small(a), Some(StarlarkIntRef::Small(b))) => Ok(Value::new_int(a ^ b)),
            (a, Some(b)) => Ok(heap.alloc(a ^ b)),
        }
    }

    fn bit_not(&self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match self.as_int_ref() {
            StarlarkIntRef::Small(a) => Ok(Value::new_int(!a)),
            big => Ok(heap.alloc(!big)),
        }
    }

    fn left_shift(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match StarlarkIntRef::unpack(other) {
            None => ValueError::unsupported_with(self, "<<", other),
            Some(other) => Ok(heap.alloc(self.as_int_ref().left_shift(other)?)),
        }
    }

    fn right_shift(&self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match StarlarkIntRef::unpack(other) {
            None => ValueError::unsupported_with(self, ">>", other),
            Some(other) => Ok(heap.alloc(self.as_int_ref().right_shift(other)?)),
        }
    }

    fn bin_op_ty(op: TypingBinOp, rhs: &TyBasic) -> Option<Ty> {
        typecheck_num_bin_op(NumTy::Int, op, rhs)
    }

    fn typechecker_ty(&self) -> Option<Ty> {
        Some(Ty::int())
    }
}
