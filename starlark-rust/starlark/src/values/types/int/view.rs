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
use crate::typing::OperatorSignature;
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
use crate::values::types::num::typecheck::num_bin_op_signatures;
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
    #[inline]
    pub fn to_integer<I>(self) -> crate::Result<I>
    where
        I: TryFrom<i32> + TryFrom<&'v BigInt>,
    {
        #[cold]
        #[inline(never)]
        fn too_big<'v, I>(int: IntView<'v>) -> crate::Error {
            crate::Error::new_value(IntegerTooBigError {
                integer_type: std::any::type_name::<I>(),
                value: int.to_string(),
            })
        }

        // A big integer is rare and its conversion is a loop: kept out of the callers, which
        // are the conversions of parameters, inlined into every native function that takes a
        // fixed-width integer.
        #[cold]
        #[inline(never)]
        fn big<'v, I: TryFrom<&'v BigInt>>(
            int: IntView<'v>,
            x: &'v StarlarkBigInt,
        ) -> crate::Result<I> {
            x.unpack_integer().ok_or_else(|| too_big::<I>(int))
        }

        match self.as_int_ref() {
            StarlarkIntRef::Small(x) => match I::try_from(x.to_i32()) {
                Ok(x) => Ok(x),
                Err(_) => Err(too_big::<I>(self)),
            },
            StarlarkIntRef::Big(x) => big(self, x),
        }
    }
}

#[starlark_value(type = INT_TYPE)]
impl<'v> StarlarkType<'v> for IntView<'v> {
    fn to_bool(this: Self) -> bool {
        this.as_int_ref() != 0
    }

    fn equals(this: Self, other: Value<'v>) -> crate::Result<bool> {
        Ok(Some(this.num()) == other.unpack_num())
    }

    fn compare(this: Self, other: Value<'v>) -> crate::Result<Ordering> {
        match other.unpack_num() {
            None => ValueError::unsupported_with(&this, "compare", other),
            Some(other) => Ok(this.num().cmp(&other)),
        }
    }

    fn write_hash(this: Self, hasher: &mut StarlarkHasher) -> crate::Result<()> {
        hasher.write_u64(this.num().get_hash_64());
        Ok(())
    }

    fn get_hash(this: Self, _private: Private) -> crate::Result<StarlarkHashValue> {
        Ok(this.num().get_hash())
    }

    fn plus(this: Self, _heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(this.to_value())
    }

    fn minus(this: Self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(heap.alloc(-this.as_int_ref()).to_value())
    }

    fn add(this: Self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(
            (this.num() + other.unpack_num()?)
                .map(|x| heap.alloc(x).to_value())
                .map_err(Into::into),
        )
    }

    fn sub(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc((this.num() - other)?).to_value()),
            None => ValueError::unsupported_with(&this, "-", other),
        }
    }

    fn mul(this: Self, other: Value<'v>, heap: Heap<'v>) -> Option<crate::Result<Value<'v>>> {
        Some(
            (this.num() * other.unpack_num()?)
                .map(|x| heap.alloc(x).to_value())
                .map_err(Into::into),
        )
    }

    fn div(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc(this.num().div(other)?).to_value()),
            None => ValueError::unsupported_with(&this, "/", other),
        }
    }

    fn percent(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc(this.num().percent(other)?).to_value()),
            None => ValueError::unsupported_with(&this, "%", other),
        }
    }

    fn floor_div(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match other.unpack_num() {
            Some(other) => Ok(heap.alloc(this.num().floor_div(other)?).to_value()),
            None => ValueError::unsupported_with(&this, "//", other),
        }
    }

    fn bit_and(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match (this.as_int_ref(), StarlarkIntRef::unpack(other)) {
            (_, None) => ValueError::unsupported_with(&this, "&", other),
            (StarlarkIntRef::Small(a), Some(StarlarkIntRef::Small(b))) => Ok(Value::new_int(a & b)),
            (a, Some(b)) => Ok(heap.alloc(a & b).to_value()),
        }
    }

    fn bit_or(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match (this.as_int_ref(), StarlarkIntRef::unpack(other)) {
            (_, None) => ValueError::unsupported_with(&this, "|", other),
            (StarlarkIntRef::Small(a), Some(StarlarkIntRef::Small(b))) => Ok(Value::new_int(a | b)),
            (a, Some(b)) => Ok(heap.alloc(a | b).to_value()),
        }
    }

    fn bit_xor(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match (this.as_int_ref(), StarlarkIntRef::unpack(other)) {
            (_, None) => ValueError::unsupported_with(&this, "^", other),
            (StarlarkIntRef::Small(a), Some(StarlarkIntRef::Small(b))) => Ok(Value::new_int(a ^ b)),
            (a, Some(b)) => Ok(heap.alloc(a ^ b).to_value()),
        }
    }

    fn bit_not(this: Self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match this.as_int_ref() {
            StarlarkIntRef::Small(a) => Ok(Value::new_int(!a)),
            big => Ok(heap.alloc(!big).to_value()),
        }
    }

    fn left_shift(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match StarlarkIntRef::unpack(other) {
            None => ValueError::unsupported_with(&this, "<<", other),
            Some(other) => Ok(heap.alloc(this.as_int_ref().left_shift(other)?).to_value()),
        }
    }

    fn right_shift(this: Self, other: Value<'v>, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match StarlarkIntRef::unpack(other) {
            None => ValueError::unsupported_with(&this, ">>", other),
            Some(other) => Ok(heap.alloc(this.as_int_ref().right_shift(other)?).to_value()),
        }
    }

    fn bin_op_signatures(op: TypingBinOp) -> Vec<OperatorSignature> {
        num_bin_op_signatures(NumTy::Int, op)
    }
}
