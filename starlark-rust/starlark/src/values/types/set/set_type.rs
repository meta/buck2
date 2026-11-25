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

use dupe::Clone_;
use dupe::Copy_;
use dupe::Dupe_;

use crate as starlark;
use crate::__derive_refs::representations::ViewOfRepresentations;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::set::SetRef;
use crate::values::set::value::SetView;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;

/// A set type marker.
///
/// [`StarlarkTypeExpr`] provides `set[T]`.
/// [`UnpackValue`] implementation verifies the types of items.
#[derive(ProvidesStaticType)]
pub struct SetType<T> {
    _item: PhantomData<T>,
}

impl<'v, T: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for SetType<T> {
    type Get = SetOf<'v, T>;

    fn starlark_type_repr() -> Ty {
        Ty::set(T::starlark_type_repr())
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        let set = SetView::from_value(value)?;
        set.to_ref().iter().all(|x| T::matches(x)).then(|| SetOf {
            value: ValueOf::new_unchecked(value),
            set,
        })
    }
}

/// Every value is a `typing.Any`, so no change to the set's items can invalidate the check.
impl<'v> StarlarkTypeExprStable<'v> for SetType<Value<'v>> {}

impl<'v, T: StarlarkTypeExpr<'v>> UnpackValue<'v> for SetType<T> {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, _get: SetOf<'v, T>) -> Result<Self, Infallible> {
        Ok(SetType { _item: PhantomData })
    }
}

impl<'v, T: StarlarkTypeExpr<'v>> ValueOf<'v, SetType<T>> {
    /// The set.
    #[inline]
    pub fn set(self) -> SetRef<'v> {
        SetOf::of_handle(self).set()
    }

    /// The items, as [`SetOf::iter`] gives them.
    pub fn iter(self) -> impl Iterator<Item = crate::Result<ValueOf<'v, T>>> + 'v
    where
        T: 'v,
    {
        SetOf::of_handle(self).iter()
    }

    /// The items, each converted by `U`, as [`SetOf::try_items`] gives them.
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        SetOf::of_handle(self).try_items()
    }
}

/// What a value read as a `set[T]` gives ([`SetType`]'s `Get`): the handle, and which of its
/// representations the set is, so that what holds one does not look the set up again. It
/// does not hold a borrow of a mutable set: [`set`](Self::set) takes one when it is called.
///
/// The type is interim: a set is to have one public type, and this is not its name.
#[derive(Clone_, Copy_, Dupe_)]
pub struct SetOf<'v, T: StarlarkTypeExpr<'v>> {
    value: ValueOf<'v, SetType<T>>,
    set: SetView<'v>,
}

impl<'v, T: StarlarkTypeExpr<'v>> SetOf<'v, T> {
    /// The reading of a handle. The set is looked up; its items are not checked.
    #[inline]
    pub(crate) fn of_handle(value: ValueOf<'v, SetType<T>>) -> Self {
        let set = SetView::from_value(value.to_value()).expect("a value's type does not change");
        SetOf { value, set }
    }

    /// The handle.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, SetType<T>> {
        self.value
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.value.to_value()
    }

    /// The set.
    #[inline]
    pub fn set(self) -> SetRef<'v> {
        self.set.to_ref()
    }

    /// The items, re-checked unless the set is frozen, as for
    /// [`StarlarkList::iter`](crate::values::list::StarlarkList::iter).
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

    /// The items, each converted by `U`, a conversion from `T`. Each is re-checked, as for
    /// [`iter`](Self::iter).
    pub fn try_items<U>(self) -> impl Iterator<Item = crate::Result<U>> + 'v
    where
        U: UnpackValue<'v, Type = T> + 'v,
    {
        self.items_raw().map(U::unpack_value_err)
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

#[cfg(test)]
mod tests {
    use crate::assert;
    use crate::values::UnpackValue;
    use crate::values::ValueOf;
    use crate::values::int::IntView;
    use crate::values::set::SetRef;
    use crate::values::set::SetType;

    #[test]
    fn test_reading() {
        let set = assert::pass("set([1, 2, 3])");
        let set = set.as_ref().value();
        let (handle, reading) = ValueOf::<SetType<IntView>>::new_with_get(set).unwrap();
        assert!(reading.handle().to_value().ptr_eq(set));
        assert_eq!(3, reading.set().iter().count());
        assert_eq!(3, reading.iter().count());
        assert_eq!(3, handle.iter().count());
        // A conversion that ends in the set is given the set the reading found.
        let unpacked = SetRef::unpack_value_opt(set).unwrap();
        assert_eq!(3, unpacked.iter().count());
    }
}
