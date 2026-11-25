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
use crate::values::dict::DictMut;
use crate::values::dict::DictRef;
use crate::values::dict::value::DictView;
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
        let dict = DictView::from_value(value)?;
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

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> ValueOf<'v, DictType<K, V>> {
    /// The empty dict.
    pub fn empty_dict() -> Self {
        Self::new_unchecked(Value::new_empty_dict())
    }

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

/// What a value read as a `dict[K, V]` gives ([`DictType`]'s `Get`): the handle, and which of
/// its representations the dict is, so that what holds one does not look the dict up again.
/// It does not hold a borrow of a mutable dict: [`dict`](Self::dict) takes one when it is
/// called.
///
/// The type is interim: a dict is to have one public type, and this is not its name.
#[derive(Clone_, Copy_, Dupe_)]
pub struct DictOf<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> {
    value: ValueOf<'v, DictType<K, V>>,
    dict: DictView<'v>,
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> DictOf<'v, K, V> {
    /// The reading of a handle. The dict is looked up; its entries are not checked.
    #[inline]
    pub(crate) fn of_handle(value: ValueOf<'v, DictType<K, V>>) -> Self {
        let dict = DictView::from_value(value.to_value()).expect("a value's type does not change");
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
        DictEntries {
            frozen: self.to_value().is_frozen(),
            entries: self.entries(),
            _marker: PhantomData,
        }
    }

    /// The entries, each converted by `K2`, a conversion from `K`, and `V2`, a conversion from
    /// `V`. Each is re-checked, as for [`iter`](Self::iter).
    pub fn try_items<K2, V2>(self) -> impl Iterator<Item = crate::Result<(K2, V2)>> + 'v
    where
        K2: UnpackValue<'v, Type = K>,
        V2: UnpackValue<'v, Type = V>,
    {
        self.entries()
            .map(|(k, v)| Ok((K2::unpack_value_err(k)?, V2::unpack_value_err(v)?)))
    }

    fn entries(self) -> Entries<'v> {
        Entries {
            dict: self.dict(),
            index: 0,
        }
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

/// The entries of a [`ValueOf<DictType<K, V>>`], borrowed from the dict.
pub struct DictEntries<'v, K, V> {
    frozen: bool,
    entries: Entries<'v>,
    _marker: PhantomData<fn() -> (K, V)>,
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>> Iterator for DictEntries<'v, K, V> {
    type Item = crate::Result<(ValueOf<'v, K>, ValueOf<'v, V>)>;

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.entries.next()?;
        Some(if self.frozen {
            Ok((ValueOf::new_unchecked(k), ValueOf::new_unchecked(v)))
        } else {
            (|| Ok((ValueOf::new_err(k)?, ValueOf::new_err(v)?)))()
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.entries.size_hint()
    }
}

/// The entries of a dict, borrowed from it; indexed because the borrow lives in the iterator.
struct Entries<'v> {
    dict: DictRef<'v>,
    index: usize,
}

impl<'v> Iterator for Entries<'v> {
    type Item = (Value<'v>, Value<'v>);

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.dict.content.get_index(self.index)?;
        self.index += 1;
        Some((*k, *v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let rem = self.dict.content.len() - self.index;
        (rem, Some(rem))
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
