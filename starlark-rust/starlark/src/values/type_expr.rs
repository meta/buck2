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

use crate::typing::Ty;
use crate::values::AllocValue;
use crate::values::Heap;
use crate::values::StarlarkType;
use crate::values::Value;
use crate::values::ValueOf;

/// A Rust type that stands for a Starlark type expression.
///
/// The implementers are the statically nameable part of Starlark's type language, realized in
/// Rust's: a nominal type is its [`StarlarkType`] impl (a single-representation type is its
/// own, a type with several representations is its view), `list[T]` is
/// [`ListType<T>`](crate::values::list::ListType) and the other containers likewise, `A | B`
/// is `Either<A, B>` or a `#[derive(UnpackValue)]` enum, `None | T` is
/// [`NoneOr<T>`](crate::values::none::NoneOr), `typing.Any` is [`Value`], and so on. Each
/// Starlark type has one implementer, so that two Rust spellings never have to be reconciled;
/// unions are the exception, since a union can be spelled in several orders. The handles to a
/// value (`ValueTyped<T>`, `&T`, `ListRef` and the like) are what a type is read through, not
/// a type.
///
/// Membership and inspection are one operation: [`get`](Self::get) says whether a value is of
/// the type by producing the handle it is read through, so nothing that is not a type can
/// implement this. An `int` that may not fit an `i32`, or a plugin kind that must be bound,
/// is a conversion from a type, which is [`UnpackValue`](crate::values::UnpackValue)'s job.
/// `bool` is the exception: a total conversion that also stands for `bool`, kept because a
/// conversion that cannot fail does not blur the check and its users are everywhere.
///
/// A Rust type that is only a conversion, such as `i32` or `Vec<T>`, is not one; it names the
/// type expression it converts from or to as its [`UnpackValue::Type`](crate::values::UnpackValue::Type)
/// or [`AllocValue::Type`].
pub trait StarlarkTypeExpr<'v>: Sized {
    /// The type as a user would write it in a type annotation, which the typechecker and the
    /// documentation go by, and which [`get`](Self::get) agrees with.
    fn starlark_type_repr() -> Ty;

    /// What a value of this type is read through: `ValueTyped<'v, Foo>` for a nominal type with
    /// one representation, the view for one with several, the untyped collection for a
    /// container (its typed items come from the [`ValueOf`] handle),
    /// `Option<T::Get>` for `NoneOr<T>`, `Either<A::Get, B::Get>` for a union.
    type Get;

    /// `value` as this type's [`Get`](Self::Get), if it is of this type: exactly what a Starlark
    /// type annotation of [`starlark_type_repr`](Self::starlark_type_repr) accepts at runtime,
    /// and nothing stricter.
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
/// `list` is. A [`ValueOf`] of such a type is read without re-checking.
pub trait StarlarkTypeExprStable<'v>: StarlarkTypeExpr<'v> {}

impl<'v, T: StarlarkType<'v>> StarlarkTypeExpr<'v> for T {
    type Get = <T as StarlarkType<'v>>::Get;

    fn starlark_type_repr() -> Ty {
        <T as StarlarkType<'v>>::get_type_starlark_repr()
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        <T as StarlarkType<'v>>::get(value)
    }
}

impl<'v, T: StarlarkType<'v>> StarlarkTypeExprStable<'v> for T {}

impl<'v> StarlarkTypeExpr<'v> for Value<'v> {
    type Get = Value<'v>;

    fn starlark_type_repr() -> Ty {
        Ty::any()
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        Some(value)
    }
}

impl<'v> StarlarkTypeExprStable<'v> for Value<'v> {}

impl<'v, TLeft: StarlarkTypeExpr<'v>, TRight: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v>
    for Either<TLeft, TRight>
{
    type Get = Either<TLeft::Get, TRight::Get>;

    fn starlark_type_repr() -> Ty {
        Ty::union2(TLeft::starlark_type_repr(), TRight::starlark_type_repr())
    }

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

/// What a variant of a `#[derive(UnpackValue)]` union holds: a [`ValueOf`] of a type
/// expression, or a type expression that is its own [`Get`](StarlarkTypeExpr::Get), such as a
/// view. The derive reads the type expression off the payload to test the variant.
pub trait UnionPayload<'v>: Sized {
    /// The type expression the payload stands for.
    type Expr: StarlarkTypeExpr<'v>;

    /// The payload, if `value` is of the type.
    fn from_value(value: Value<'v>) -> Option<Self>;
}

impl<'v, T: StarlarkTypeExpr<'v>> UnionPayload<'v> for ValueOf<'v, T> {
    type Expr = T;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<Self> {
        ValueOf::new(value)
    }
}

/// The type expressions that are their own handle are payloads as themselves. (A blanket impl
/// over `Get = Self` does not normalize for a `T` that carries `'v`, so each says so.)
macro_rules! union_payload_is_self {
    ($(impl[$($g:tt)*] $t:ty $(where [$($w:tt)*])?;)*) => {$(
        impl<$($g)*> UnionPayload<'v> for $t $(where $($w)*)? {
            type Expr = $t;

            #[inline]
            fn from_value(value: Value<'v>) -> Option<Self> {
                <$t as StarlarkTypeExpr<'v>>::get(value)
            }
        }
    )*};
}

union_payload_is_self! {
    impl['v] Value<'v>;
    impl['v] bool;
    impl['v] crate::values::structs::StructRef<'v>;
    impl['v] crate::values::enumeration::AnyEnumType<'v>;
    impl['v] crate::values::typing::TypeType;
    impl['v, P: crate::values::typing::StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>]
        crate::values::typing::StarlarkCallable<'v, P, R>;
    impl['v, P: crate::values::typing::StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>]
        crate::values::typing::StarlarkCallableChecked<'v, P, R>;
}

/// `None` carries nothing, so the unit value is as good a payload as its handle.
impl<'v> UnionPayload<'v> for crate::values::none::NoneType {
    type Expr = crate::values::none::NoneType;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<Self> {
        value.is_none().then_some(crate::values::none::NoneType)
    }
}

/// `typing.Never`: nothing is of it, so its handle has no values either.
impl<'v> StarlarkTypeExpr<'v> for crate::values::typing::StarlarkNever {
    type Get = Infallible;

    fn starlark_type_repr() -> Ty {
        Ty::never()
    }

    fn get(_value: Value<'v>) -> Option<Self::Get> {
        None
    }
}

impl<'v> StarlarkTypeExprStable<'v> for crate::values::typing::StarlarkNever {}

/// Derive macros generate a reference to this method to be able to get the `type_repr` of types
/// they can't name
#[doc(hidden)]
pub fn type_repr_from_attr_impl<'v, T: AllocValue<'v>, V, E>(
    _f: fn(V, Heap<'v>) -> Result<T, E>,
) -> Ty {
    T::Type::starlark_type_repr()
}

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
            agrees::<StarlarkStr>(&vs);
            agrees::<IntView>(&vs);
            agrees::<Value>(&vs);
            agrees::<ListType<IntView>>(&vs);
            agrees::<ListType<StarlarkStr>>(&vs);
            agrees::<DictType<IntView, StarlarkStr>>(&vs);
            agrees::<DictType<StarlarkStr, IntView>>(&vs);
            agrees::<TupleType<IntView>>(&vs);
            agrees::<(IntView, IntView)>(&vs);
            agrees::<(IntView, StarlarkStr)>(&vs);
            agrees::<NoneOr<StarlarkStr>>(&vs);
            agrees::<Either<bool, StarlarkStr>>(&vs);
            agrees::<StarlarkIter<IntView>>(&vs);
        });
    }
}
