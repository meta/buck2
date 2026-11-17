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

use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;
use either::Either;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::list::ListRef;
use crate::values::tuple::TupleType;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;

/// A list type marker.
///
/// [`StarlarkTypeExpr`] provides `list[T]`.
/// [`UnpackValue`] implementation verifies the types of items.
#[derive(ProvidesStaticType)]
pub struct ListType<T> {
    _item: std::marker::PhantomData<T>,
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for ListType<T> {
    type Get = ListOf<'v, T>;

    fn starlark_type_repr() -> Ty {
        Ty::list(T::starlark_type_repr())
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        let list = ListRef::from_value(value)?;
        list.iter().all(|x| T::matches(x)).then(|| ListOf {
            value: ValueOf::new_unchecked(value),
            list,
        })
    }
}

/// Every value is a `typing.Any`, so no change to the list's items can invalidate the check.
impl<'v> StarlarkTypeExprStable<'v> for ListType<Value<'v>> {}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for ListType<T> {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, _get: ListOf<'v, T>) -> Result<Self, Infallible> {
        Ok(ListType {
            _item: std::marker::PhantomData,
        })
    }
}

/// A type that `[]` belongs to: [`ListType`] and
/// [`ListOrTupleType`](crate::values::list_or_tuple::ListOrTupleType).
pub trait EmptyListType: sealed::Sealed {}

impl<T> EmptyListType for ListType<T> {}
impl<T> EmptyListType for Either<ListType<T>, TupleType<T>> {}

mod sealed {
    use either::Either;

    use crate::values::list::ListType;
    use crate::values::tuple::TupleType;

    pub trait Sealed {}
    impl<T> Sealed for ListType<T> {}
    impl<T> Sealed for Either<ListType<T>, TupleType<T>> {}
}

impl<'v, T: StarlarkTypeExpr<'v> + EmptyListType> ValueOf<'v, T> {
    /// The empty list.
    pub fn empty_list() -> Self {
        Self::new_unchecked(Value::new_empty_list())
    }
}

/// What a value read as a `list[T]` gives ([`ListType`]'s `Get`): the handle, and the list
/// that the read found, so that what holds one does not look the list up again.
///
/// The type is interim: a list is to have one public type, and this is not its name.
#[derive(Clone_, Copy_, Dupe_)]
pub struct ListOf<'v, T: StarlarkTypeExpr<'v>> {
    value: ValueOf<'v, ListType<T>>,
    list: &'v ListRef<'v>,
}

impl<'v, T: StarlarkTypeExpr<'v>> ListOf<'v, T> {
    /// The reading of a handle. The list is looked up; its items are not checked.
    #[inline]
    pub(crate) fn of_handle(value: ValueOf<'v, ListType<T>>) -> Self {
        let list = ListRef::from_value(value.to_value()).expect("a value's type does not change");
        ListOf { value, list }
    }

    /// The reading of a handle whose list has been looked up already.
    #[inline]
    pub(crate) fn of_handle_and_list(
        value: ValueOf<'v, ListType<T>>,
        list: &'v ListRef<'v>,
    ) -> Self {
        ListOf { value, list }
    }

    /// The handle.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, ListType<T>> {
        self.value
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.value.to_value()
    }

    /// The list.
    #[inline]
    pub fn list(self) -> &'v ListRef<'v> {
        self.list
    }

    /// The elements. Each is re-checked unless the list is frozen, since a mutable list may
    /// have been changed after the check; for a frozen list every item is `Ok`.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        let frozen = self.to_value().is_frozen();
        self.list.iter().map(move |x| {
            if frozen {
                Ok(ValueOf::new_unchecked(x))
            } else {
                ValueOf::new_err(x)
            }
        })
    }

    /// The items, each converted by `U`, a conversion from `T`. Each is re-checked, as for
    /// [`iter`](Self::iter).
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.list.iter().map(U::unpack_value_err)
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.try_items().collect()
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, ListType<T>> {
    /// The list.
    #[inline]
    pub fn list(self) -> &'v ListRef<'v> {
        ListOf::of_handle(self).list()
    }

    /// The elements, as [`ListOf::iter`] gives them.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        ListOf::of_handle(self).iter()
    }

    /// The items, each converted by `U`, as [`ListOf::try_items`] gives them.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        ListOf::of_handle(self).try_items()
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        ListOf::of_handle(self).try_to_vec()
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::UnpackValue;
    use crate::values::ValueOf;
    use crate::values::int::IntView;
    use crate::values::list::AllocList;
    use crate::values::list::ListRef;
    use crate::values::list::ListType;

    #[test]
    fn test_reading() {
        Heap::temp(|heap| {
            let list = heap.alloc(AllocList([1, 2, 3])).to_value();
            let (handle, reading) = ValueOf::<ListType<IntView>>::new_with_get(list).unwrap();
            assert!(reading.handle().to_value().ptr_eq(list));
            assert!(std::ptr::eq(handle.list(), reading.list()));
            assert_eq!(3, reading.iter().count());
            assert_eq!(vec![1, 2, 3], reading.try_to_vec::<i32>().unwrap());
            // A conversion that ends in the list is given the list the reading found.
            let unpacked = <&ListRef>::unpack_value_opt(list).unwrap();
            assert!(std::ptr::eq(handle.list(), unpacked));
        });
    }
}
