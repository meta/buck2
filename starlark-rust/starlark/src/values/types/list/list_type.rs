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

use either::Either;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::list::AllocList;
use crate::values::list::ListRef;
use crate::values::tuple::TupleType;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_repr::StarlarkTypeRepr;

/// A list type marker.
///
/// [`StarlarkTypeRepr`] provides `list[T]`.
/// [`UnpackValue`] implementation verifies the types of items.
#[derive(ProvidesStaticType)]
pub struct ListType<T> {
    _item: std::marker::PhantomData<T>,
}

impl<T: StarlarkTypeRepr> StarlarkTypeRepr for ListType<T> {
    type Canonical = ListType<T::Canonical>;

    fn starlark_type_repr() -> Ty {
        Ty::list(T::starlark_type_repr())
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for ListType<T> {
    type Get = &'v ListRef<'v>;

    fn get(value: Value<'v>) -> Option<Self::Get> {
        ListRef::from_value(value).filter(|l| l.iter().all(|x| T::matches(x)))
    }
}

impl<'v, T: UnpackValue<'v>> UnpackValue<'v> for ListType<T> {
    type Error = <T as UnpackValue<'v>>::Error;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        let Some(list) = ListRef::from_value(value) else {
            return Ok(None);
        };
        for item in list.iter() {
            if T::unpack_value_impl(item)?.is_none() {
                return Ok(None);
            }
        }
        Ok(Some(ListType {
            _item: std::marker::PhantomData,
        }))
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

impl<'v, T: UnpackValue<'v> + EmptyListType> ValueOf<'v, T> {
    /// The empty list.
    pub fn empty_list() -> Self {
        Self::new_unchecked(Value::new_empty_list())
    }
}

impl<'v, T: UnpackValue<'v>> ValueOf<'v, ListType<T>> {
    /// A new list of these values.
    pub fn alloc_list(heap: Heap<'v>, items: impl IntoIterator<Item = ValueOf<'v, T>>) -> Self {
        Self::new_unchecked(heap.alloc(AllocList(items.into_iter().map(|x| x.to_value()))))
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
        T: 'v,
    {
        self.list().iter().map(T::unpack_value_err)
    }

    /// The items, unpacked into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec(self) -> crate::Result<Vec<T>>
    where
        T: 'v,
    {
        self.try_items().collect()
    }
}
