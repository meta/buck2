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

//! What `#[starlark_value]` calls for a [`StarlarkType`] method written with a typed signature.
//!
//! The macro turns the typed method `foo` into the trait's method `foo`, whose body is one call
//! to `typed_foo` here, handing it the typed body as a closure. Each function converts the
//! operand and says what a value that is not of the operand's type means for its method, which
//! is what the trait's default for the method does.
//!
//! For the typechecker, the macro reads the same signature: the functions at the end give what
//! a typed operator's operand and result types say.

use std::cmp::Ordering;

use crate::collections::StarlarkHasher;
use crate::typing::OperatorSignature;
use crate::typing::Ty;
use crate::values::AllocValue;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::StarlarkTypeExpr;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueError;
use crate::values::unpack_then;

/// What a typed method may return where the trait's method returns `starlark::Result<T>`: a
/// `T`, for a body that cannot fail, or the `starlark::Result<T>`.
#[diagnostic::on_unimplemented(
    message = "a typed `StarlarkType` method returns `{T}` or `starlark::Result<{T}>` here, not `{Self}`"
)]
pub trait TypedResult<T> {
    fn into_result(self) -> crate::Result<T>;
}

impl<T> TypedResult<T> for T {
    #[inline(always)]
    fn into_result(self) -> crate::Result<T> {
        Ok(self)
    }
}

impl<T> TypedResult<T> for crate::Result<T> {
    #[inline(always)]
    fn into_result(self) -> crate::Result<T> {
        self
    }
}

/// What a typed operator may return where the trait's method returns a [`Value`]: something
/// that makes a value, for a body that cannot fail, or a `starlark::Result` of it.
#[diagnostic::on_unimplemented(
    message = "a typed operator returns an `AllocValue` or a `starlark::Result` of one, not `{Self}`"
)]
pub trait TypedOperatorResult<'v> {
    /// The Starlark type of the value made: the result type of the operator.
    type Type: StarlarkTypeExpr<'v>;

    fn alloc_result(self, heap: Heap<'v>) -> crate::Result<Value<'v>>;
}

impl<'v, T: AllocValue<'v>> TypedOperatorResult<'v> for T {
    type Type = T::Type;

    #[inline(always)]
    fn alloc_result(self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        Ok(heap.alloc(self).to_value())
    }
}

impl<'v, T: AllocValue<'v>> TypedOperatorResult<'v> for crate::Result<T> {
    type Type = T::Type;

    #[inline(always)]
    fn alloc_result(self, heap: Heap<'v>) -> crate::Result<Value<'v>> {
        match self {
            Ok(x) => Ok(heap.alloc(x).to_value()),
            Err(e) => Err(e),
        }
    }
}

/// [`StarlarkType::equals`] from a typed body. A value that is not of the operand's type is not
/// equal.
#[inline(always)]
pub fn typed_equals<'v, O, R>(other: Value<'v>, typed: impl FnOnce(O) -> R) -> crate::Result<bool>
where
    O: UnpackValue<'v>,
    R: TypedResult<bool>,
{
    unpack_then(other, |other| typed(other).into_result(), || Ok(false), Err)
}

/// [`StarlarkType::compare`] from a typed body. A value that is not of the operand's type does
/// not compare: [`compare_unsupported`].
#[inline(always)]
pub fn typed_compare<'v, T, O, R>(
    this: &T,
    other: Value<'v>,
    typed: impl FnOnce(O) -> R,
) -> crate::Result<Ordering>
where
    T: StarlarkType<'v>,
    O: UnpackValue<'v>,
    R: TypedResult<Ordering>,
{
    unpack_then(
        other,
        |other| typed(other).into_result(),
        || compare_unsupported(this, other),
        Err,
    )
}

/// The error for comparing `this` with a value it does not compare with: the default of
/// [`StarlarkType::compare`], and the answer of a typed `compare` for a value that is not of
/// its operand's type.
#[cold]
pub fn compare_unsupported<'v, T: StarlarkType<'v>>(
    this: &T,
    other: Value<'v>,
) -> crate::Result<Ordering> {
    ValueError::unsupported_with(this, "compare", other)
}

/// [`StarlarkType::write_hash`] from a body that cannot fail.
#[inline(always)]
pub fn typed_write_hash(
    hasher: &mut StarlarkHasher,
    typed: impl FnOnce(&mut StarlarkHasher),
) -> crate::Result<()> {
    typed(hasher);
    Ok(())
}

/// The operators that leave a value they do not take to the other operand, each from a typed
/// body. A value that is not of the operand's type is `None`, as the default is.
macro_rules! typed_operators_or_none {
    ($( $method:ident => $typed:ident; )*) => {
        $(
            #[doc = concat!(
                "[`StarlarkType::", stringify!($method), "`] from a typed body. A value that ",
                "is not of the operand's type is `None`, which gives the other operand its turn.",
            )]
            #[inline(always)]
            pub fn $typed<'v, O, R>(
                other: Value<'v>,
                heap: Heap<'v>,
                typed: impl FnOnce(O, Heap<'v>) -> R,
            ) -> Option<crate::Result<Value<'v>>>
            where
                O: UnpackValue<'v>,
                R: TypedOperatorResult<'v>,
            {
                unpack_then(
                    other,
                    |other| Some(typed(other, heap).alloc_result(heap)),
                    || None,
                    |e| Some(Err(e)),
                )
            }
        )*
    };
}

typed_operators_or_none! {
    add => typed_add;
    radd => typed_radd;
    mul => typed_mul;
    rmul => typed_rmul;
}

/// The operators that are an error for a value they do not take. For each, the trait's method
/// from a typed body, and the error: the default of the method, and the answer of a typed one
/// for a value that is not of its operand's type.
macro_rules! typed_operators {
    ($( $method:ident, $symbol:literal => $typed:ident, $unsupported:ident; )*) => {
        $(
            #[doc = concat!(
                "[`StarlarkType::", stringify!($method), "`] from a typed body. A value that ",
                "is not of the operand's type is not supported: [`", stringify!($unsupported),
                "`].",
            )]
            #[inline(always)]
            pub fn $typed<'v, T, O, R>(
                this: &T,
                other: Value<'v>,
                heap: Heap<'v>,
                typed: impl FnOnce(O, Heap<'v>) -> R,
            ) -> crate::Result<Value<'v>>
            where
                T: StarlarkType<'v>,
                O: UnpackValue<'v>,
                R: TypedOperatorResult<'v>,
            {
                unpack_then(
                    other,
                    |other| typed(other, heap).alloc_result(heap),
                    || $unsupported(this, other),
                    Err,
                )
            }

            #[doc = concat!(
                "The error for `this ", $symbol, " other` with an `other` that `this` does not ",
                "take: the default of [`StarlarkType::", stringify!($method), "`], and the ",
                "answer of a typed one for a value that is not of its operand's type.",
            )]
            #[cold]
            pub fn $unsupported<'v, T: StarlarkType<'v>>(
                this: &T,
                other: Value<'v>,
            ) -> crate::Result<Value<'v>> {
                ValueError::unsupported_with(this, $symbol, other)
            }
        )*
    };
}

typed_operators! {
    sub, "-" => typed_sub, sub_unsupported;
    div, "/" => typed_div, div_unsupported;
    percent, "%" => typed_percent, percent_unsupported;
    floor_div, "//" => typed_floor_div, floor_div_unsupported;
    bit_and, "&" => typed_bit_and, bit_and_unsupported;
    bit_or, "|" => typed_bit_or, bit_or_unsupported;
    bit_xor, "^" => typed_bit_xor, bit_xor_unsupported;
    left_shift, "<<" => typed_left_shift, left_shift_unsupported;
    right_shift, ">>" => typed_right_shift, right_shift_unsupported;
}

/// The unary operators, each from a typed body.
macro_rules! typed_unary_operators {
    ($( $method:ident => $typed:ident; )*) => {
        $(
            #[doc = concat!("[`StarlarkType::", stringify!($method), "`] from a typed body.")]
            #[inline(always)]
            pub fn $typed<'v, R: TypedOperatorResult<'v>>(
                heap: Heap<'v>,
                typed: impl FnOnce(Heap<'v>) -> R,
            ) -> crate::Result<Value<'v>> {
                typed(heap).alloc_result(heap)
            }
        )*
    };
}

typed_unary_operators! {
    plus => typed_plus;
    minus => typed_minus;
    bit_not => typed_bit_not;
}

/// What a typed binary operator says to the typechecker: an operand of the type `O` converts
/// from gives a result of the type `R` makes.
pub fn operator_signature<'v, O, R>() -> OperatorSignature
where
    O: UnpackValue<'v>,
    R: TypedOperatorResult<'v>,
{
    OperatorSignature {
        operand: O::Type::starlark_type_repr(),
        result: R::Type::starlark_type_repr(),
    }
}

/// What a typed `compare` says to the typechecker about `<` and its like: the operand is of the
/// type `O` converts from.
pub fn compare_signature<'v, O: UnpackValue<'v>>() -> OperatorSignature {
    OperatorSignature {
        operand: O::Type::starlark_type_repr(),
        result: Ty::bool(),
    }
}

/// What a typed unary operator says to the typechecker: the result is of the type `R` makes.
pub fn unary_operator_result<'v, R: TypedOperatorResult<'v>>() -> Ty {
    R::Type::starlark_type_repr()
}
