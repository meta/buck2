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

use std::collections::BTreeMap;

use mini_map::MiniMutInsertionMap;

use crate::collections::SmallMap;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::ValueOf;
use crate::values::dict::AllocDict;
use crate::values::types::dict::dict_type::DictType;

// MiniMutInsertionMap: the map behind `SmallMap`, for callers that name it directly.

impl<'v, K: AllocValue<'v>, V: AllocValue<'v>> AllocValue<'v> for MiniMutInsertionMap<K, V> {
    type Type = DictType<K::Type, V::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, K: AllocFrozenValue<'fv>, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for MiniMutInsertionMap<K, V>
{
    type Type = DictType<K::Type, V::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

// SmallMap

impl<'v, K: AllocValue<'v>, V: AllocValue<'v>> AllocValue<'v> for SmallMap<K, V> {
    type Type = DictType<K::Type, V::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, K: AllocFrozenValue<'fv>, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for SmallMap<K, V>
{
    type Type = DictType<K::Type, V::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<'a, 'v, K: 'a, V: 'a> AllocValue<'v> for &'a SmallMap<K, V>
where
    &'a K: AllocValue<'v>,
    &'a V: AllocValue<'v>,
{
    type Type = DictType<<&'a K as AllocValue<'v>>::Type, <&'a V as AllocValue<'v>>::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, 'a, K: 'a, V: 'a> AllocFrozenValue<'fv> for &'a SmallMap<K, V>
where
    &'a K: AllocFrozenValue<'fv>,
    &'a V: AllocFrozenValue<'fv>,
{
    type Type =
        DictType<<&'a K as AllocFrozenValue<'fv>>::Type, <&'a V as AllocFrozenValue<'fv>>::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

// BTreeMap

impl<'v, K: AllocValue<'v>, V: AllocValue<'v>> AllocValue<'v> for BTreeMap<K, V> {
    type Type = DictType<K::Type, V::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, K: AllocFrozenValue<'fv>, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for BTreeMap<K, V>
{
    type Type = DictType<K::Type, V::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<'a, 'v, K: 'a, V: 'a> AllocValue<'v> for &'a BTreeMap<K, V>
where
    &'a K: AllocValue<'v>,
    &'a V: AllocValue<'v>,
{
    type Type = DictType<<&'a K as AllocValue<'v>>::Type, <&'a V as AllocValue<'v>>::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, 'a, K: 'a, V: 'a> AllocFrozenValue<'fv> for &'a BTreeMap<K, V>
where
    &'a K: AllocFrozenValue<'fv>,
    &'a V: AllocFrozenValue<'fv>,
{
    type Type =
        DictType<<&'a K as AllocFrozenValue<'fv>>::Type, <&'a V as AllocFrozenValue<'fv>>::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}
