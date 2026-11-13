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
use std::marker::PhantomData;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::Heap;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::tuple::TupleRef;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;

/// A tuple type marker: a tuple of any length whose items are all `T`.
///
/// [`StarlarkTypeExpr`] provides `tuple[T, ...]`.
/// [`UnpackValue`] implementation verifies the types of items.
#[derive(ProvidesStaticType)]
pub struct TupleType<T> {
    _item: PhantomData<T>,
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for TupleType<T> {
    /// The handle: a typed tuple has no cheaper form that keeps `T`.
    type Get = ValueOf<'v, TupleType<T>>;

    fn starlark_type_repr() -> Ty {
        Ty::tuple_of(T::starlark_type_repr())
    }

    fn get(value: Value<'v>) -> Option<Self::Get> {
        let tuple = TupleRef::from_value(value)?;
        tuple
            .iter()
            .all(|x| T::matches(x))
            .then(|| ValueOf::new_unchecked(value))
    }
}

/// A tuple's items are fixed, so the check is as stable as the items' own.
impl<'v, T: StarlarkTypeExprStable<'v>> StarlarkTypeExprStable<'v> for TupleType<T> {}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for TupleType<T> {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, _get: ValueOf<'v, Self>) -> Result<Self, Infallible> {
        Ok(TupleType { _item: PhantomData })
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, TupleType<T>> {
    /// A new tuple of these values.
    pub fn alloc_tuple(heap: Heap<'v>, items: impl IntoIterator<Item = ValueOf<'v, T>>) -> Self {
        Self::new_unchecked(heap.alloc_tuple_iter(items.into_iter().map(|x| x.to_value())))
    }

    /// The tuple.
    #[inline]
    pub fn tuple(self) -> &'v TupleRef<'v> {
        TupleRef::from_value(self.to_value()).expect("a value's type does not change")
    }

    /// The items. Each is re-checked unless the tuple is frozen: the tuple itself cannot
    /// change, but an item that is a mutable container can; for a frozen tuple every item is
    /// `Ok`.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v {
        let frozen = self.to_value().is_frozen();
        self.tuple().iter().map(move |x| {
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
        self.tuple()
            .iter()
            .map(|x| ValueOf::<T>::new_unchecked(x).get())
    }

    /// The items, each converted by `U`, a conversion from `T`. Each is re-checked, as for
    /// [`iter`](Self::iter).
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.tuple().iter().map(U::unpack_value_err)
    }

    /// The items, converted into a `Vec`, as [`try_items`](Self::try_items) gives them.
    pub fn try_to_vec<U>(self) -> crate::Result<Vec<U>>
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.try_items().collect()
    }
}
