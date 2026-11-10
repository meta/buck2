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

use either::Either;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::Heap;
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
    /// The handle: a typed list has no cheaper form that keeps `T`.
    type Get = ValueOf<'v, ListType<T>>;

    fn starlark_type_repr() -> Ty {
        Ty::list(T::starlark_type_repr())
    }

    fn get(value: Value<'v>) -> Option<Self::Get> {
        let list = ListRef::from_value(value)?;
        list.iter()
            .all(|x| T::matches(x))
            .then(|| ValueOf::new_unchecked(value))
    }
}

/// Every value is a `typing.Any`, so no change to the list's items can invalidate the check.
impl<'v> StarlarkTypeExprStable<'v> for ListType<Value<'v>> {}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for ListType<T> {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, _get: ValueOf<'v, Self>) -> Result<Self, Infallible> {
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

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, ListType<T>> {
    /// A new list of these values.
    pub fn alloc_list(heap: Heap<'v>, items: impl IntoIterator<Item = ValueOf<'v, T>>) -> Self {
        Self::new_unchecked(heap.alloc_list_iter(items.into_iter().map(|x| x.to_value())))
    }

    /// The list.
    #[inline]
    pub fn list(self) -> &'v ListRef<'v> {
        ListRef::from_value(self.to_value()).expect("a value's type does not change")
    }

    /// The elements. Each is re-checked unless the list is frozen, since a mutable list may
    /// have been changed after the check; for a frozen list every item is `Ok`.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        let frozen = self.to_value().is_frozen();
        self.list().iter().map(move |x| {
            if frozen {
                Ok(ValueOf::new_unchecked(x))
            } else {
                ValueOf::new_err(x)
            }
        })
    }

    /// The items, unpacked. Each is re-checked, as for [`iter`](Self::iter).
    pub fn try_items(self) -> impl Iterator<Item = crate::Result<T>> + 'v
    where
        T: UnpackValue<'v> + 'v,
    {
        self.list().iter().map(T::unpack_value_err)
    }

    /// The items, unpacked into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec(self) -> crate::Result<Vec<T>>
    where
        T: UnpackValue<'v> + 'v,
    {
        self.try_items().collect()
    }
}
