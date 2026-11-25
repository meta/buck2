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
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::DictEntries;
use crate::values::dict::DictMut;
use crate::values::dict::DictRef;
use crate::values::dict::StarlarkDict;
use crate::values::dict::value::DictRepr;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::type_expr::StarlarkTypeExprStable;

/// A dict type marker.
///
/// [`StarlarkTypeExpr`] provides `dict[K, V]`.
/// [`UnpackValue`] implementation verifies the types of entries and discards them.
#[derive(ProvidesStaticType)]
pub struct DictType<K, V> {
    k: PhantomData<K>,
    v: PhantomData<V>,
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> StarlarkTypeExpr<'v> for DictType<K, V> {
    type Get = DictOf<'v, K, V>;

    fn starlark_type_repr() -> Ty {
        Ty::dict(K::starlark_type_repr(), V::starlark_type_repr())
    }

    #[inline]
    fn get(value: Value<'v>) -> Option<Self::Get> {
        let dict = DictRepr::of_value(value)?;
        dict.to_ref()
            .iter()
            .all(|(k, v)| K::matches(k) && V::matches(v))
            .then(|| DictOf {
                value: ValueOf::new_unchecked(value),
                dict,
            })
    }
}

/// Every value is a `typing.Any`, so no change to the dict's entries can invalidate the check.
impl<'v> StarlarkTypeExprStable<'v> for DictType<Value<'v>, Value<'v>> {}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> UnpackValue<'v> for DictType<K, V> {
    type Type = Self;
    type Error = Infallible;

    fn unpack(_value: ValueOf<'v, Self>, _get: DictOf<'v, K, V>) -> Result<Self, Infallible> {
        Ok(DictType {
            k: PhantomData,
            v: PhantomData,
        })
    }
}

/// A type that `{}` belongs to: [`StarlarkDict`], and [`DictType`] while it still exists.
pub trait EmptyDictType: sealed::Sealed {}

impl<K, V> EmptyDictType for DictType<K, V> {}
impl<'v, K, V> EmptyDictType for StarlarkDict<'v, K, V> {}

mod sealed {
    use crate::values::dict::DictType;
    use crate::values::dict::StarlarkDict;

    pub trait Sealed {}
    impl<K, V> Sealed for DictType<K, V> {}
    impl<'v, K, V> Sealed for StarlarkDict<'v, K, V> {}
}

impl<'v, T: StarlarkTypeExpr<'v> + EmptyDictType> ValueOf<'v, T> {
    /// The empty dict.
    pub fn empty_dict() -> Self {
        Self::new_unchecked(Value::new_empty_dict())
    }
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> ValueOf<'v, DictType<K, V>> {
    /// The dict.
    #[inline]
    pub fn dict(self) -> DictRef<'v> {
        DictOf::of_handle(self).dict()
    }

    /// The dict, to change it: an error if it is frozen or being iterated.
    pub fn dict_mut(self) -> crate::Result<DictMut<'v>> {
        DictOf::of_handle(self).dict_mut()
    }

    /// The entries, as [`DictOf::iter`] gives them.
    pub fn iter(self) -> DictEntries<'v, K, V> {
        DictOf::of_handle(self).iter()
    }

    /// The entries, each converted by `K2` and `V2`, as [`DictOf::try_items`] gives them.
    pub fn try_items<K2, V2>(self) -> impl Iterator<Item = crate::Result<(K2, V2)>> + 'v
    where
        K2: UnpackValue<'v, Type = K>,
        V2: UnpackValue<'v, Type = V>,
    {
        DictOf::of_handle(self).try_items()
    }

    /// The value at `key`, re-checked unless the dict is frozen.
    pub fn lookup(self, key: Value<'v>) -> crate::Result<Option<ValueOf<'v, V>>> {
        DictOf::of_handle(self).lookup(key)
    }

    /// The value at a string key, re-checked unless the dict is frozen.
    pub fn lookup_str(self, key: &str) -> crate::Result<Option<ValueOf<'v, V>>> {
        DictOf::of_handle(self).lookup_str(key)
    }
}

/// A handle to a `dict[K, V]` under either of its two names is a handle under the other, for
/// code that has one and is given the other while [`DictType`] still exists.
impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> From<ValueOf<'v, DictType<K, V>>>
    for ValueOf<'v, StarlarkDict<'v, K, V>>
{
    #[inline]
    fn from(dict: ValueOf<'v, DictType<K, V>>) -> Self {
        // The same type under its other name.
        ValueOf::new_unchecked(dict.to_value())
    }
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> From<ValueOf<'v, StarlarkDict<'v, K, V>>>
    for ValueOf<'v, DictType<K, V>>
{
    #[inline]
    fn from(dict: ValueOf<'v, StarlarkDict<'v, K, V>>) -> Self {
        // The same type under its other name.
        ValueOf::new_unchecked(dict.to_value())
    }
}

/// What a value read as a `dict[K, V]` gives ([`DictType`]'s `Get`): the handle, and which of
/// its representations the dict is, so that what holds one does not look the dict up again.
/// It does not hold a borrow of a mutable dict: [`dict`](Self::dict) takes one when it is
/// called.
///
/// The type is interim: a dict is to have one public type, and this is not its name.
#[derive(Clone_, Copy_, Dupe_)]
pub struct DictOf<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> {
    value: ValueOf<'v, DictType<K, V>>,
    dict: DictRepr<'v>,
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> DictOf<'v, K, V> {
    /// The reading of a handle. The dict is looked up; its entries are not checked.
    #[inline]
    pub(crate) fn of_handle(value: ValueOf<'v, DictType<K, V>>) -> Self {
        let dict = DictRepr::of_value(value.to_value()).expect("a value's type does not change");
        DictOf { value, dict }
    }

    /// The handle.
    #[inline]
    pub fn handle(self) -> ValueOf<'v, DictType<K, V>> {
        self.value
    }

    /// The value.
    #[inline]
    pub fn to_value(self) -> Value<'v> {
        self.value.to_value()
    }

    /// The dict.
    #[inline]
    pub fn dict(self) -> DictRef<'v> {
        self.dict.to_ref()
    }

    /// The dict, to change it: an error if it is frozen or being iterated.
    pub fn dict_mut(self) -> crate::Result<DictMut<'v>> {
        Ok(DictMut::from_value(self.to_value())?)
    }

    /// The entries, re-checked unless the dict is frozen, as for
    /// [`StarlarkList::iter`](crate::values::list::StarlarkList::iter).
    pub fn iter(self) -> DictEntries<'v, K, V> {
        DictEntries::of_borrowed(self.dict(), self.to_value().is_frozen())
    }

    /// The entries, each converted by `K2`, a conversion from `K`, and `V2`, a conversion from
    /// `V`. Each is re-checked, as for [`iter`](Self::iter).
    pub fn try_items<K2, V2>(self) -> impl Iterator<Item = crate::Result<(K2, V2)>> + 'v
    where
        K2: UnpackValue<'v, Type = K>,
        V2: UnpackValue<'v, Type = V>,
    {
        StarlarkDict::<'v, K, V>::of_repr(self.dict).try_items()
    }

    /// The value at `key`, re-checked unless the dict is frozen.
    pub fn lookup(self, key: Value<'v>) -> crate::Result<Option<ValueOf<'v, V>>> {
        self.dict().get(key)?.map(|v| self.checked(v)).transpose()
    }

    /// The value at a string key, re-checked unless the dict is frozen.
    pub fn lookup_str(self, key: &str) -> crate::Result<Option<ValueOf<'v, V>>> {
        self.dict()
            .get_str(key)
            .map(|v| self.checked(v))
            .transpose()
    }

    fn checked(self, v: Value<'v>) -> crate::Result<ValueOf<'v, V>> {
        if self.to_value().is_frozen() {
            Ok(ValueOf::new_unchecked(v))
        } else {
            ValueOf::new_err(v)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::UnpackValue;
    use crate::values::ValueOf;
    use crate::values::dict::AllocDict;
    use crate::values::dict::DictMut;
    use crate::values::dict::DictRef;
    use crate::values::dict::DictType;
    use crate::values::int::IntView;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_reading() {
        Heap::temp(|heap| {
            let dict = heap.alloc(AllocDict([("a", 1), ("b", 2)])).to_value();
            let (handle, reading) =
                ValueOf::<DictType<StarlarkStr, IntView>>::new_with_get(dict).unwrap();
            assert!(reading.handle().to_value().ptr_eq(dict));
            assert_eq!(2, reading.dict().len());
            assert_eq!(2, reading.iter().count());
            assert_eq!(2, handle.iter().count());
            assert!(reading.lookup_str("a").unwrap().is_some());
            // The reading holds no borrow, so the dict can be changed while one is held.
            DictMut::from_value(dict)
                .unwrap()
                .aref
                .insert_hashed(heap.alloc("c").to_value().get_hashed().unwrap(), dict);
            assert_eq!(3, reading.dict().len());
            // A conversion that ends in the dict borrows it.
            let unpacked = DictRef::unpack_value_opt(dict).unwrap();
            assert_eq!(3, unpacked.len());
            assert!(DictMut::from_value(dict).is_err());
        });
    }
}
