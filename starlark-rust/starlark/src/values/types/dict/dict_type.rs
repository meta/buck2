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

use either::Either;

use crate as starlark;
use crate::any::ProvidesStaticType;
use crate::typing::Ty;
use crate::values::Heap;
use crate::values::UnpackAndDiscard;
use crate::values::UnpackValue;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::AllocDict;
use crate::values::dict::DictRef;
use crate::values::dict::UnpackDictEntries;
use crate::values::type_repr::StarlarkTypeRepr;

/// A dict type marker.
///
/// [`StarlarkTypeRepr`] provides `dict[K, V]`.
/// [`UnpackValue`] implementation verifies the types of entries and discards them.
#[derive(ProvidesStaticType)]
pub struct DictType<K: StarlarkTypeRepr, V: StarlarkTypeRepr> {
    k: PhantomData<K>,
    v: PhantomData<V>,
}

impl<K: StarlarkTypeRepr, V: StarlarkTypeRepr> StarlarkTypeRepr for DictType<K, V> {
    type Canonical = DictType<K::Canonical, V::Canonical>;

    fn starlark_type_repr() -> Ty {
        Ty::dict(K::starlark_type_repr(), V::starlark_type_repr())
    }
}

impl<'v, K: UnpackValue<'v>, V: UnpackValue<'v>> UnpackValue<'v> for DictType<K, V> {
    type Error = Either<K::Error, V::Error>;
    const UNCHECKED: bool = K::UNCHECKED || V::UNCHECKED;

    fn unpack_value_impl(value: Value<'v>) -> Result<Option<Self>, Self::Error> {
        match UnpackDictEntries::<UnpackAndDiscard<K>, UnpackAndDiscard<V>>::unpack_value_impl(
            value,
        ) {
            Ok(Some(_)) => Ok(Some(DictType {
                k: PhantomData,
                v: PhantomData,
            })),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl<'v, K: UnpackValue<'v>, V: UnpackValue<'v>> ValueOf<'v, DictType<K, V>> {
    /// A new dict of these entries.
    pub fn alloc_dict(
        heap: Heap<'v>,
        entries: impl IntoIterator<Item = (ValueOf<'v, K>, ValueOf<'v, V>)>,
    ) -> Self {
        Self::new_unchecked(
            heap.alloc(AllocDict(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.to_value(), v.to_value())),
            )),
        )
    }

    /// The dict.
    #[inline]
    pub fn dict(self) -> DictRef<'v> {
        DictRef::from_value(self.to_value()).expect("a value's type does not change")
    }

    /// The entries, re-checked unless the dict is frozen, as for
    /// [`ValueOf<ListType<T>>::iter`](ValueOf::iter).
    pub fn iter(
        self,
    ) -> impl Iterator<Item = crate::Result<(ValueOf<'v, K>, ValueOf<'v, V>)>> + 'v {
        let frozen = self.to_value().is_frozen();
        let entries: Vec<(Value<'v>, Value<'v>)> = self.dict().iter().collect();
        entries.into_iter().map(move |(k, v)| {
            if frozen {
                Ok((ValueOf::new_unchecked(k), ValueOf::new_unchecked(v)))
            } else {
                Ok((ValueOf::new_err(k)?, ValueOf::new_err(v)?))
            }
        })
    }

    /// The value at a string key, re-checked unless the dict is frozen.
    pub fn get_str(self, key: &str) -> crate::Result<Option<ValueOf<'v, V>>> {
        match self.dict().get_str(key) {
            None => Ok(None),
            Some(v) if self.to_value().is_frozen() => Ok(Some(ValueOf::new_unchecked(v))),
            Some(v) => Ok(Some(ValueOf::new_err(v)?)),
        }
    }
}
