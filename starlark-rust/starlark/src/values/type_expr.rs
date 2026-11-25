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
use crate::values::Value;
use crate::values::ValueOf;

/// A Rust type that stands for a Starlark type expression.
///
/// The implementers are the statically nameable part of Starlark's type language, realized in
/// Rust's: a nominal type is its [`StarlarkType`](crate::values::StarlarkType) impl (a
/// single-representation type is its own, a type with several representations is its view,
/// `bool` is the Rust `bool`), for which
/// [`#[starlark_value]`](crate::values::starlark_value) writes this impl,
/// `list[T]` is [`StarlarkList<'v, T>`](crate::values::list::StarlarkList) and the other
/// containers likewise, `A | B` is `Either<A, B>` or a `#[derive(UnpackValue)]` enum, `None | T` is
/// [`NoneOr<T>`](crate::values::none::NoneOr), `typing.Any` is [`Value`], and so on. Each
/// Starlark type has one implementer, so that two Rust spellings never have to be reconciled;
/// unions are the exception, since a union can be spelled in several orders. The handles to a
/// value (`ValueTyped<T>`, `&T`, `&TupleRef` and the like) are what a type is read through,
/// not a type.
///
/// Membership and inspection are one operation: [`get`](Self::get) says whether a value is of
/// the type by producing the handle it is read through, so nothing that is not a type can
/// implement this. An `int` that may not fit an `i32`, or a plugin kind that must be bound,
/// is a conversion from a type, which is [`UnpackValue`](crate::values::UnpackValue)'s job.
///
/// A Rust type that is only a conversion, such as `i32` or `Vec<T>`, is not one; it names the
/// type expression it converts from or to as its [`UnpackValue::Type`](crate::values::UnpackValue::Type)
/// or [`AllocValue::Type`].
pub trait StarlarkTypeExpr<'v>: Sized {
    /// The type as a user would write it in a type annotation, which the typechecker and the
    /// documentation go by, and which [`get`](Self::get) agrees with.
    fn starlark_type_repr() -> Ty;

    /// What a value of this type is read through: `ValueTyped<'v, Foo>` for a nominal type with
    /// one representation, the view for one with several, the type itself for a list
    /// ([`StarlarkList`](crate::values::list::StarlarkList)), the reading for another typed
    /// container (the [`ValueOf`] handle together with what the read found, such as
    /// [`TupleOf`](crate::values::tuple::TupleOf)), `Option<T::Get>` for `NoneOr<T>`,
    /// `Either<A::Get, B::Get>` for a union.
    ///
    /// For a [`StarlarkType`](crate::values::StarlarkType) it is a handle: it is copied freely
    /// and converts back into the value it was read from. It is what the type's methods take
    /// as their receiver.
    type Get;

    /// `value` as this type's [`Get`](Self::Get), if it is of this type: exactly what a Starlark
    /// type annotation of [`starlark_type_repr`](Self::starlark_type_repr) accepts at runtime,
    /// and nothing stricter.
    fn get(value: Value<'v>) -> Option<Self::Get>;

    /// Whether `value` is of this type: `get(value).is_some()`. An impl does not override
    /// this; [`get`](Self::get) alone decides what is of the type.
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
///
/// A nominal type's check is stable, so this is a supertrait of
/// [`StarlarkType`](crate::values::StarlarkType), and
/// [`#[starlark_value]`](crate::values::starlark_value) writes the impl.
pub trait StarlarkTypeExprStable<'v>: StarlarkTypeExpr<'v> {}

/// A [`StarlarkTypeExpr`] whose values all hash:
/// [`write_hash`](crate::values::StarlarkType::write_hash) of a value of the type cannot fail.
///
/// A type has this by writing its `write_hash` without a result in its
/// [`#[starlark_value]`](crate::values::starlark_value) block, which implements it; there is
/// nothing to implement by hand for a type. A union has it when both sides do, and a tuple
/// type when its items do.
///
/// It is what lets a handle hash its value without a `Result`:
/// [`ValueOf::write_hash`](crate::values::ValueOf::write_hash).
pub trait StarlarkTypeExprHashable<'v>: StarlarkTypeExpr<'v> {}

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

impl<'v, TLeft: StarlarkTypeExprHashable<'v>, TRight: StarlarkTypeExprHashable<'v>>
    StarlarkTypeExprHashable<'v> for Either<TLeft, TRight>
{
}

/// What a variant of a `#[derive(UnpackValue)]` union holds: a [`ValueOf`] of a type
/// expression, a type expression that is its own [`Get`](StarlarkTypeExpr::Get), such as a
/// view, or a reference that a type is read through, such as `&TupleRef`. The derive reads the
/// type expression off the payload to test the variant.
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
    impl['v] crate::values::typing::TypeType;
    impl['v, P: crate::values::typing::StarlarkCallableParamSpec, R: StarlarkTypeExpr<'static>]
        crate::values::typing::StarlarkCallable<'v, P, R>;
}

/// A conversion that cannot fail and gives the reference its type is read through is a payload
/// as well: the variant then holds what the read found, and the arm does not read again.
macro_rules! union_payload_is_conversion {
    ($($t:ty;)*) => {$(
        impl<'v> UnionPayload<'v> for $t {
            type Expr = <$t as crate::values::UnpackValue<'v>>::Type;

            #[inline]
            fn from_value(value: Value<'v>) -> Option<Self> {
                <$t as crate::values::UnpackValue<'v>>::unpack_value_opt(value)
            }
        }
    )*};
}

union_payload_is_conversion! {
    &'v str;
    &'v crate::values::tuple::TupleRef<'v>;
    crate::values::dict::DictRef<'v>;
    crate::values::set::SetRef<'v>;
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

    use crate::eval::compiler::def::Def;
    use crate::typing::Ty;
    use crate::values::Heap;
    use crate::values::Value;
    use crate::values::dict::AllocDict;
    use crate::values::dict::DictType;
    use crate::values::list::AllocList;
    use crate::values::list::StarlarkList;
    use crate::values::none::NoneOr;
    use crate::values::none::NoneType;
    use crate::values::string::str_type::StarlarkStr;
    use crate::values::tuple::StarlarkTuple;
    use crate::values::type_expr::StarlarkTypeExpr;
    use crate::values::type_expr::StarlarkTypeExprHashable;
    use crate::values::types::bytes::value::StarlarkBytes;
    use crate::values::types::dict::value::DictView;
    use crate::values::types::float::StarlarkFloat;
    use crate::values::types::int::view::IntView;
    use crate::values::types::record::field::Field;
    use crate::values::types::record::record_type::RecordTypeView;
    use crate::values::types::set::value::SetData;
    use crate::values::types::set::value::SetView;
    use crate::values::typing::StarlarkIter;
    use crate::values::typing::TypeCompiled;
    use crate::values::typing::type_compiled::matcher::TypeMatcher;
    use crate::values::unpack::type_matcher;

    fn values<'v>(heap: Heap<'v>) -> Vec<Value<'v>> {
        vec![
            Value::new_none(),
            Value::new_bool(true),
            heap.alloc(1i32).to_value(),
            heap.alloc(BigInt::from(1u64 << 40)).to_value(),
            heap.alloc(1.5).to_value(),
            heap.alloc("s").to_value(),
            heap.alloc(AllocList(vec![1i32, 2i32])).to_value(),
            heap.alloc(AllocList(vec!["a"])).to_value(),
            heap.alloc(AllocList(Vec::<i32>::new())).to_value(),
            heap.alloc(AllocDict([(1i32, "a")])).to_value(),
            heap.alloc(AllocDict([("a", 1i32)])).to_value(),
            heap.alloc((1i32, 2i32)).to_value(),
            heap.alloc((1i32, "a")).to_value(),
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

    /// A value of a type that knows nothing finer about its values is typed as the type.
    #[test]
    fn test_ty_of_value_is_the_type() {
        #[track_caller]
        fn typed_as<'v, T: StarlarkTypeExpr<'v>>(value: Value<'v>) {
            assert_eq!(Ty::of_value(value), T::starlark_type_repr(), "`{value}`");
        }

        Heap::temp(|heap| {
            typed_as::<NoneType>(Value::new_none());
            typed_as::<bool>(Value::new_bool(true));
            typed_as::<IntView>(heap.alloc(1i32).to_value());
            typed_as::<IntView>(heap.alloc(BigInt::from(1u64 << 40)).to_value());
            typed_as::<StarlarkFloat>(heap.alloc(1.5).to_value());
            typed_as::<StarlarkStr>(heap.alloc("s").to_value());
            typed_as::<StarlarkBytes>(heap.alloc(StarlarkBytes::new(b"b")).to_value());
            typed_as::<StarlarkList>(heap.alloc(AllocList([1i32])).to_value());
            typed_as::<DictView>(heap.alloc(AllocDict([(1i32, "a")])).to_value());
            typed_as::<SetView>(heap.alloc(SetData::default()).to_value());
            typed_as::<StarlarkTuple>(heap.alloc((1i32, "a")).to_value());
            let field = Field::new(TypeCompiled::from_ty(&Ty::int(), heap), None);
            typed_as::<Field>(heap.alloc(field).to_value());
        });
    }

    /// The types whose `write_hash` is written without a result, and what is built from them.
    #[test]
    fn test_types_that_always_hash() {
        fn always_hashes<'v, T: StarlarkTypeExprHashable<'v>>() {}

        always_hashes::<bool>();
        always_hashes::<NoneType>();
        always_hashes::<StarlarkStr>();
        always_hashes::<StarlarkBytes>();
        always_hashes::<TypeCompiled>();
        always_hashes::<RecordTypeView>();
        always_hashes::<Def>();
        always_hashes::<Either<bool, StarlarkStr>>();
        always_hashes::<(StarlarkStr, NoneType)>();
        always_hashes::<StarlarkTuple<StarlarkBytes>>();
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
            agrees::<StarlarkList<IntView>>(&vs);
            agrees::<StarlarkList<StarlarkStr>>(&vs);
            agrees::<DictType<IntView, StarlarkStr>>(&vs);
            agrees::<DictType<StarlarkStr, IntView>>(&vs);
            agrees::<StarlarkTuple<IntView>>(&vs);
            agrees::<(IntView, IntView)>(&vs);
            agrees::<(IntView, StarlarkStr)>(&vs);
            agrees::<NoneOr<StarlarkStr>>(&vs);
            agrees::<Either<bool, StarlarkStr>>(&vs);
            agrees::<StarlarkIter<IntView>>(&vs);
        });
    }
}
