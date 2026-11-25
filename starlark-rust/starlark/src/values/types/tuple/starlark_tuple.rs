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

use std::convert::Infallible;
use std::fmt;
use std::fmt::Debug;
use std::fmt::Display;
use std::iter;
use std::marker::PhantomData;
use std::slice;

use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;

use crate::any::IsStaticType;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprHashable;
use crate::values::type_expr::StarlarkTypeExprStable;
use crate::values::type_expr::UnionPayload;
use crate::values::types::tuple::value::Tuple;
use crate::values::types::tuple::value::VALUE_EMPTY_TUPLE;

/// A Starlark tuple. `StarlarkTuple<'v>` is the type `tuple`, and `StarlarkTuple<'v, T>` is
/// `tuple[T, ...]`, a tuple of any length whose items are of the type `T`.
///
/// A tuple of fixed length with a type for each position, `tuple[A, B]`, is the Rust tuple
/// of those types, `(A, B)`.
///
/// # What to write where
///
/// - A parameter of a native function, the operand of a typed operator, a variant of a
///   `#[derive(UnpackValue)]` union: `StarlarkTuple<'v, T>`. It is the value read as a tuple,
///   with its items read as `T`s.
/// - A field that keeps a tuple: `ValueOf<'v, StarlarkTuple<'v, T>>`, the checked handle,
///   which is traced and frozen. It has the reading methods of this type, and
///   [`tuple`](ValueOf::tuple) gives the `StarlarkTuple`.
/// - A value that may or may not be a tuple: [`StarlarkTuple::from_value`].
/// - The items: [`content`](Self::content) as a slice of plain values,
///   [`values`](Self::values) to iterate them, [`iter`](Self::iter) as `T`s,
///   [`try_items`](Self::try_items) converted.
///
/// # A tuple does not change
///
/// So, unlike a list's, its slice can be kept for as long as the heap lives, and iterating it
/// refuses nothing.
///
/// An item may still be a list or a dict that changes. So what reads an item as a `T` checks
/// it, except in a frozen tuple, and except where `T`'s check cannot change
/// ([`items`](Self::items)).
#[derive(Clone_, Copy_, Dupe_)]
pub struct StarlarkTuple<'v, T = Value<'v>> {
    tuple: ValueTyped<'v, Tuple<'v>>,
    _item: PhantomData<fn() -> T>,
}

// SAFETY: the static type is the same type with the heap lifetime erased.
unsafe impl<'v, T: ProvidesStaticType<'v>> ProvidesStaticType<'v> for StarlarkTuple<'v, T>
where
    T::StaticType: Sized,
{
    type StaticType = StarlarkTuple<'static, T::StaticType>;
}

impl<T: IsStaticType> IsStaticType for StarlarkTuple<'static, T>
where
    for<'lt> T::Reinfect<'lt>: Sized,
{
    type Reinfect<'lt> = StarlarkTuple<'lt, T::Reinfect<'lt>>;
}

/// `tuple[T, ...]` for every `T`. At `T = Value` this is the type expression of the type
/// `tuple`, which is why the tuple's `StarlarkType` block says `type_expr = by_hand`.
impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for StarlarkTuple<'v, T> {
    type Get = Self;

    fn starlark_type_repr() -> Ty {
        Ty::tuple_of(T::starlark_type_repr())
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self> {
        let tuple = ValueTyped::<Tuple>::new(value)?;
        tuple
            .as_ref()
            .content()
            .iter()
            .all(|x| T::matches(*x))
            .then_some(StarlarkTuple::of_tuple(tuple))
    }
}

/// A tuple's items are fixed, so the check is as stable as the items' own.
impl<'v, T: StarlarkTypeExprStable<'v>> StarlarkTypeExprStable<'v> for StarlarkTuple<'v, T> {}

/// A tuple hashes its items.
impl<'v, T: StarlarkTypeExprHashable<'v>> StarlarkTypeExprHashable<'v> for StarlarkTuple<'v, T> {}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for StarlarkTuple<'v, T> {
    type Type = Self;
    type Error = Infallible;

    #[inline]
    fn unpack(_value: ValueOf<'v, Self>, get: Self) -> Result<Self, Infallible> {
        Ok(get)
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> UnionPayload<'v> for StarlarkTuple<'v, T> {
    type Expr = Self;

    #[inline]
    fn from_value(value: Value<'v>) -> Option<Self> {
        Self::get(value)
    }
}

impl<'v, T> From<StarlarkTuple<'v, T>> for Value<'v> {
    #[inline]
    fn from(tuple: StarlarkTuple<'v, T>) -> Value<'v> {
        tuple.tuple.to_value()
    }
}

impl<'v> From<ValueTyped<'v, Tuple<'v>>> for StarlarkTuple<'v> {
    #[inline]
    fn from(tuple: ValueTyped<'v, Tuple<'v>>) -> Self {
        StarlarkTuple::of_tuple(tuple)
    }
}

impl<'v> StarlarkTuple<'v> {
    /// `value` as a tuple, if it is one. For a `tuple[T, ...]`, which looks at the items, it
    /// is [`StarlarkTypeExpr::get`], or [`ValueOf::new`] for the handle.
    #[inline]
    pub fn from_value(value: Value<'v>) -> Option<Self> {
        ValueTyped::new(value).map(StarlarkTuple::of_tuple)
    }
}

impl<'v, T> StarlarkTuple<'v, T> {
    /// The tuple. Its items are not looked at: the caller read them as `T`s, or `T` is
    /// `Value`.
    #[inline]
    pub(crate) fn of_tuple(tuple: ValueTyped<'v, Tuple<'v>>) -> Self {
        StarlarkTuple {
            tuple,
            _item: PhantomData,
        }
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.tuple.to_value()
    }

    /// This tuple as a tuple of anything, for what takes any tuple.
    #[inline]
    pub fn untyped(self) -> StarlarkTuple<'v> {
        StarlarkTuple::of_tuple(self.tuple)
    }

    /// The number of items.
    #[inline]
    pub fn len(self) -> usize {
        self.content().len()
    }

    /// Whether the tuple has no items.
    #[inline]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The items, as plain values.
    #[inline]
    pub fn content(self) -> &'v [Value<'v>] {
        self.tuple.as_ref().content()
    }

    /// The items, as plain values.
    #[inline]
    pub fn values(self) -> iter::Copied<slice::Iter<'v, Value<'v>>> {
        self.content().iter().copied()
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTuple<'v, T> {
    /// The empty tuple, which needs no heap.
    #[inline]
    pub fn empty() -> Self {
        Self::of_tuple(VALUE_EMPTY_TUPLE.at())
    }

    /// The handle to this tuple, for a field or a return.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, Self> {
        // The items were `T`s when this was read, which is all a handle says.
        ValueOf::new_unchecked(self.to_value())
    }

    /// The items. Each is checked to be a `T` as it is read unless the tuple is frozen: the
    /// tuple itself cannot change, but an item that is a mutable container can; in a frozen
    /// tuple nothing is checked and every item is `Ok`.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        let frozen = self.to_value().is_frozen();
        self.values().map(move |x| {
            if frozen {
                Ok(ValueOf::new_unchecked(x))
            } else {
                ValueOf::new_err(x)
            }
        })
    }

    /// The items, read as [`get`](ValueOf::get) reads them. A tuple does not change, so for a
    /// `T` whose check cannot change either there is nothing that can fail.
    pub fn items(self) -> impl Iterator<Item = T::Get> + 'v
    where
        T: StarlarkTypeExprStable<'v> + 'v,
    {
        self.values()
            .map(|x| T::get(x).expect("an item of a tuple checked as a `tuple[T, ...]` is a `T`"))
    }

    /// The items, each converted by `U`, a conversion from `T`. Each is checked, as for
    /// [`iter`](Self::iter).
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.values().map(U::unpack_value_err)
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.try_items().collect()
    }
}

impl<'v, T> Debug for StarlarkTuple<'v, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("StarlarkTuple").field(&self.tuple).finish()
    }
}

impl<'v, T> Display for StarlarkTuple<'v, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self.tuple.as_ref(), f)
    }
}

/// The handle to a tuple reads it as the tuple does: these are [`StarlarkTuple`]'s methods of
/// the same names, so that a field is read without a step in between.
impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, StarlarkTuple<'v, T>> {
    /// The tuple. Its items are not checked here: what reads them as `T`s checks each, as
    /// [`StarlarkTuple::iter`] says. ([`try_get`](ValueOf::try_get) is this with every item
    /// checked first.)
    #[inline]
    pub fn tuple(self) -> StarlarkTuple<'v, T> {
        StarlarkTuple::of_tuple(
            ValueTyped::new(self.to_value()).expect("a value's type does not change"),
        )
    }

    /// [`StarlarkTuple::len`].
    #[inline]
    pub fn len(self) -> usize {
        self.tuple().len()
    }

    /// [`StarlarkTuple::is_empty`].
    #[inline]
    pub fn is_empty(self) -> bool {
        self.tuple().is_empty()
    }

    /// [`StarlarkTuple::content`].
    #[inline]
    pub fn content(self) -> &'v [Value<'v>] {
        self.tuple().content()
    }

    /// [`StarlarkTuple::values`].
    #[inline]
    pub fn values(self) -> iter::Copied<slice::Iter<'v, Value<'v>>> {
        self.tuple().values()
    }

    /// [`StarlarkTuple::iter`].
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        self.tuple().iter()
    }

    /// [`StarlarkTuple::items`].
    pub fn items(self) -> impl Iterator<Item = T::Get> + 'v
    where
        T: StarlarkTypeExprStable<'v> + 'v,
    {
        self.tuple().items()
    }

    /// [`StarlarkTuple::try_items`].
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.tuple().try_items()
    }

    /// [`StarlarkTuple::try_to_vec`].
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.tuple().try_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::StarlarkTypeExpr;
    use crate::values::UnpackValue;
    use crate::values::ValueOf;
    use crate::values::int::IntView;
    use crate::values::string::StarlarkStr;
    use crate::values::tuple::AllocTuple;
    use crate::values::tuple::StarlarkTuple;

    #[test]
    fn test_reading() {
        Heap::temp(|heap| {
            let tuple = heap.alloc(AllocTuple([1, 2, 3])).to_value();
            let (handle, reading) = ValueOf::<StarlarkTuple<IntView>>::new_with_get(tuple).unwrap();
            assert!(reading.to_value().ptr_eq(tuple));
            assert!(reading.handle().to_value().ptr_eq(tuple));
            assert_eq!(3, reading.len());
            assert_eq!(3, reading.iter().count());
            assert_eq!(3, reading.items().count());
            assert_eq!(vec![1, 2, 3], reading.try_to_vec::<i32>().unwrap());
            // The handle reads the tuple as the tuple does.
            assert_eq!(3, handle.len());
            assert_eq!(vec![1, 2, 3], handle.try_to_vec::<i32>().unwrap());
            assert!(std::ptr::eq(handle.content(), reading.content()));
            // A parameter spelled as the type is the reading.
            let unpacked = StarlarkTuple::from_value(tuple).unwrap();
            assert!(std::ptr::eq(reading.content(), unpacked.content()));
            assert!(<StarlarkTuple<StarlarkStr>>::unpack_value_opt(tuple).is_none());
        });
    }

    /// `StarlarkTuple<'v>` is the type `tuple` and `tuple[typing.Any, ...]`; they are one type.
    #[test]
    fn test_the_tuple_type_and_a_tuple_of_anything_are_one() {
        assert_eq!(
            <StarlarkTuple as StarlarkTypeExpr>::starlark_type_repr(),
            <StarlarkTuple as crate::values::StarlarkType>::get_type_starlark_repr(),
        );
        Heap::temp(|heap| {
            let tuple = heap.alloc(AllocTuple(["a"])).to_value();
            let typed = <StarlarkTuple<StarlarkStr>>::unpack_value_opt(tuple).unwrap();
            assert!(typed.untyped().to_value().ptr_eq(tuple));
            assert!(ValueOf::<StarlarkTuple>::new(tuple).is_some());
        });
    }
}
