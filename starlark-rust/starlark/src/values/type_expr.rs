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

//! The Rust types that stand for Starlark type expressions.

use std::convert::Infallible;

use either::Either;

use crate::values::StarlarkType;
use crate::values::Value;
use crate::values::type_repr::StarlarkTypeRepr;

/// A Rust type that stands for a Starlark type expression.
///
/// The implementers are the statically nameable part of Starlark's type language, realized in
/// Rust's: a nominal type is its [`StarlarkType`] impl (a single-representation type is its
/// own, a type with several representations is its view), `list[T]` is
/// [`ListType<T>`](crate::values::list::ListType) and the other containers likewise, `A | B`
/// is `Either<A, B>` or a `#[derive(UnpackValue)]` enum, `None | T` is
/// [`NoneOr<T>`](crate::values::none::NoneOr), `typing.Any` is [`Value`], and so on. Each
/// Starlark type has one implementer, so that two Rust spellings never have to be reconciled;
/// unions are the exception, since a union can be spelled in several orders, and the
/// handles `ValueTyped<T>` and `&T` keep an impl for one representation until they stop being
/// bindings.
///
/// Membership and inspection are one operation: [`get`](Self::get) says whether a value is of
/// the type by producing the handle it is read through, so nothing that is not a type can
/// implement this. An `int` that may not fit an `i32`, or a plugin kind that must be bound,
/// is a conversion from a type, which is [`UnpackValue`](crate::values::UnpackValue)'s job.
/// `&str`, `String` and `bool` are the transitional exceptions: total conversions that keep
/// an impl so that `ListType<&str>` keeps working until the container helpers take the
/// conversion as a type argument.
///
/// Not every [`StarlarkTypeRepr`] is one: a return type such as `Vec<T>` has a `Ty` for the
/// documentation and nothing to test.
pub trait StarlarkTypeExpr<'v>: StarlarkTypeRepr {
    /// What a value of this type is read through: `ValueTyped<'v, Foo>` for a nominal type with
    /// one representation, the view for one with several, the untyped collection for a
    /// container (its typed items come from the [`ValueOf`](crate::values::ValueOf) handle),
    /// `Option<T::Get>` for `NoneOr<T>`, `Either<A::Get, B::Get>` for a union.
    type Get;

    /// `value` as this type's [`Get`](Self::Get), if it is of this type: exactly what a Starlark
    /// type annotation of [`starlark_type_repr`](StarlarkTypeRepr::starlark_type_repr) accepts
    /// at runtime, and nothing stricter.
    fn get(value: Value<'v>) -> Option<Self::Get>;

    /// Whether `value` is of this type.
    #[inline]
    fn matches(value: Value<'v>) -> bool {
        Self::get(value).is_some()
    }
}

/// A [`StarlarkTypeExpr`] whose verdict for a value cannot change over the value's lifetime.
///
/// The check depends only on what is fixed once a value exists: its type, the contents of an
/// immutable value, or a property of its type such as implementing a trait. It does not look
/// into a mutable container, whose contents Starlark may change later: `list[int]` is a finer
/// type than the representation `list`, and a check for it is not stable, while a check for
/// `list` is. A [`ValueOf`](crate::values::ValueOf) of such a type is read without re-checking.
pub trait StarlarkTypeExprStable<'v>: StarlarkTypeExpr<'v> {}

impl<'v, T: StarlarkType<'v> + StarlarkTypeRepr> StarlarkTypeExpr<'v> for T {
    type Get = <T as StarlarkType<'v>>::Get;

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        <T as StarlarkType<'v>>::get(value)
    }
}

impl<'v, T: StarlarkType<'v> + StarlarkTypeRepr> StarlarkTypeExprStable<'v> for T {}

impl<'v> StarlarkTypeExpr<'v> for Value<'v> {
    type Get = Value<'v>;

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        Some(value)
    }
}

impl<'v> StarlarkTypeExprStable<'v> for Value<'v> {}

impl<'v> StarlarkTypeExpr<'v> for String {
    type Get = String;

    fn get(value: Value<'v>) -> Option<Self::Get> {
        value.unpack_str().map(str::to_owned)
    }
}

impl<'v> StarlarkTypeExprStable<'v> for String {}

impl<'v> StarlarkTypeExpr<'v> for &'v str {
    type Get = &'v str;

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        value.unpack_str()
    }
}

impl<'v> StarlarkTypeExprStable<'v> for &'v str {}

impl<'v, TLeft: StarlarkTypeExpr<'v>, TRight: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v>
    for Either<TLeft, TRight>
{
    type Get = Either<TLeft::Get, TRight::Get>;

    fn get(value: Value<'v>) -> Option<Self::Get> {
        match TLeft::get(value) {
            Some(left) => Some(Either::Left(left)),
            None => TRight::get(value).map(Either::Right),
        }
    }
}

impl<'v, TLeft: StarlarkTypeExprStable<'v>, TRight: StarlarkTypeExprStable<'v>>
    StarlarkTypeExprStable<'v> for Either<TLeft, TRight>
{
}

/// `typing.Never`: nothing is of it, so its handle has no values either.
impl<'v> StarlarkTypeExpr<'v> for crate::values::typing::StarlarkNever {
    type Get = Infallible;

    fn get(_value: Value<'v>) -> Option<Self::Get> {
        None
    }
}

impl<'v> StarlarkTypeExprStable<'v> for crate::values::typing::StarlarkNever {}

#[cfg(test)]
mod tests {
    use either::Either;
    use num_bigint::BigInt;

    use crate::values::Heap;
    use crate::values::Value;
    use crate::values::dict::AllocDict;
    use crate::values::dict::DictType;
    use crate::values::list::ListType;
    use crate::values::none::NoneOr;
    use crate::values::none::NoneType;
    use crate::values::string::str_type::StarlarkStr;
    use crate::values::tuple::TupleType;
    use crate::values::type_expr::StarlarkTypeExpr;
    use crate::values::types::int::view::IntView;
    use crate::values::typing::StarlarkIter;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::unpack::type_matcher;

    fn values<'v>(heap: Heap<'v>) -> Vec<Value<'v>> {
        vec![
            Value::new_none(),
            Value::new_bool(true),
            heap.alloc(1),
            heap.alloc(BigInt::from(1u64 << 40)),
            heap.alloc(1.5),
            heap.alloc("s"),
            heap.alloc(vec![1, 2]),
            heap.alloc(vec!["a"]),
            heap.alloc(Vec::<i32>::new()),
            heap.alloc(AllocDict([(1, "a")])),
            heap.alloc(AllocDict([("a", 1)])),
            heap.alloc((1, 2)),
            heap.alloc((1, "a")),
        ]
    }

    /// The direct check and the one compiled from the `Ty` say the same thing.
    fn agrees<'v, T: StarlarkTypeExpr<'v>>(values: &[Value<'v>]) {
        for &value in values {
            assert_eq!(
                T::matches(value),
                type_matcher::<T>().matches(value),
                "`{}` on `{value}`",
                T::starlark_type_repr()
            );
        }
    }

    #[test]
    fn test_get_agrees_with_the_type() {
        Heap::temp(|heap| {
            let vs = values(heap);
            agrees::<bool>(&vs);
            agrees::<NoneType>(&vs);
            agrees::<String>(&vs);
            agrees::<&str>(&vs);
            agrees::<StarlarkStr>(&vs);
            agrees::<IntView>(&vs);
            agrees::<Value>(&vs);
            agrees::<ListType<IntView>>(&vs);
            agrees::<ListType<String>>(&vs);
            agrees::<DictType<IntView, String>>(&vs);
            agrees::<DictType<String, IntView>>(&vs);
            agrees::<TupleType<IntView>>(&vs);
            agrees::<(IntView, IntView)>(&vs);
            agrees::<(IntView, String)>(&vs);
            agrees::<NoneOr<String>>(&vs);
            agrees::<Either<bool, String>>(&vs);
            agrees::<StarlarkIter<IntView>>(&vs);
        });
    }
}
