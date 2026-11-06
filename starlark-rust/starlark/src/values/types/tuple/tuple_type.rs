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

use std::marker::PhantomData;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackAndDiscard;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::tuple::TupleRef;
use crate::values::tuple::UnpackTuple;
use crate::values::type_repr::StarlarkTypeRepr;

/// A tuple type marker: a tuple of any length whose items are all `T`.
///
/// [`StarlarkTypeRepr`] provides `tuple[T, ...]`.
/// [`UnpackValue`] implementation verifies the types of items.
#[derive(ProvidesStaticType)]
pub struct TupleType<T> {
    _item: PhantomData<T>,
}

impl<T: StarlarkTypeRepr> StarlarkTypeRepr for TupleType<T> {
    type Canonical = TupleType<T::Canonical>;

    fn starlark_type_repr() -> Ty {
        Ty::tuple_of(T::starlark_type_repr())
    }
}

impl<'v, T: UnpackValue<'v>> UnpackValue<'v> for TupleType<T> {
    type Error = <T as UnpackValue<'v>>::Error;
    const UNCHECKED: bool = T::UNCHECKED;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        match UnpackTuple::<UnpackAndDiscard<T>>::unpack_value_impl(value) {
            Ok(Some(_)) => Ok(Some(TupleType { _item: PhantomData })),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl<'v, T: UnpackValue<'v>> ValueOf<'v, TupleType<T>> {
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
}
