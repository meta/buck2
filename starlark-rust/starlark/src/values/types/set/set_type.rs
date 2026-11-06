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
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::set::SetRef;
use crate::values::type_repr::StarlarkTypeRepr;

/// A set type marker.
///
/// [`StarlarkTypeRepr`] provides `set[T]`.
/// [`UnpackValue`] implementation verifies the types of items.
#[derive(ProvidesStaticType)]
pub struct SetType<T> {
    _item: PhantomData<T>,
}

impl<T: StarlarkTypeRepr> StarlarkTypeRepr for SetType<T> {
    type Canonical = SetType<T::Canonical>;

    fn starlark_type_repr() -> Ty {
        Ty::set(T::starlark_type_repr())
    }
}

impl<'v, T: UnpackValue<'v>> UnpackValue<'v> for SetType<T> {
    type Error = <T as UnpackValue<'v>>::Error;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        let Some(set) = SetRef::from_value(value) else {
            return Ok(None);
        };
        for item in set.iter() {
            if T::unpack_value_impl(item)?.is_none() {
                return Ok(None);
            }
        }
        Ok(Some(SetType { _item: PhantomData }))
    }
}

impl<'v, T: UnpackValue<'v>> ValueOf<'v, SetType<T>> {
    /// The set.
    #[inline]
    pub fn set(self) -> SetRef<'v> {
        SetRef::from_value(self.to_value()).expect("a value's type does not change")
    }

    /// The items, re-checked unless the set is frozen, as for
    /// [`ValueOf<ListType<T>>::iter`](ValueOf::iter).
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v
    where
        T: 'v,
    {
        let frozen = self.to_value().is_frozen();
        self.items_raw().map(move |x| {
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
        self.items_raw().map(T::unpack_value_err)
    }

    fn items_raw(self) -> Items<'v> {
        Items {
            set: self.set(),
            index: 0,
        }
    }
}

/// The items of a set, borrowed from it; indexed because the borrow lives in the iterator.
struct Items<'v> {
    set: SetRef<'v>,
    index: usize,
}

impl<'v> Iterator for Items<'v> {
    type Item = Value<'v>;

    fn next(&mut self) -> Option<Value<'v>> {
        let x = *self.set.aref.content.get_index(self.index)?;
        self.index += 1;
        Some(x)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.set.aref.content.len() - self.index;
        (rem, Some(rem))
    }
}
