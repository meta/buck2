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
use crate::typing::Ty;
use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Value;
use crate::values::dict::AllocDict;
use crate::values::type_repr::StarlarkTypeRepr;
use crate::values::types::dict::dict_type::DictType;

// MiniMutInsertionMap: the map behind `SmallMap`, for callers that name it directly.

impl<'v, K: AllocValue<'v>, V: AllocValue<'v>> AllocValue<'v> for MiniMutInsertionMap<K, V> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, K: AllocFrozenValue<'fv>, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for MiniMutInsertionMap<K, V>
{
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<K: StarlarkTypeRepr, V: StarlarkTypeRepr> StarlarkTypeRepr for MiniMutInsertionMap<K, V> {
    type Canonical = <DictType<K, V> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        DictType::<K, V>::starlark_type_repr()
    }
}

// SmallMap

impl<'v, K: AllocValue<'v>, V: AllocValue<'v>> AllocValue<'v> for SmallMap<K, V> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, K: AllocFrozenValue<'fv>, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for SmallMap<K, V>
{
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<'a, 'v, K: 'a + StarlarkTypeRepr, V: 'a + StarlarkTypeRepr> AllocValue<'v>
    for &'a SmallMap<K, V>
where
    &'a K: AllocValue<'v>,
    &'a V: AllocValue<'v>,
{
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, 'a, K: 'a + StarlarkTypeRepr, V: 'a + StarlarkTypeRepr> AllocFrozenValue<'fv>
    for &'a SmallMap<K, V>
where
    &'a K: AllocFrozenValue<'fv>,
    &'a V: AllocFrozenValue<'fv>,
{
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<'a, K: StarlarkTypeRepr, V: StarlarkTypeRepr> StarlarkTypeRepr for &'a SmallMap<K, V> {
    type Canonical = <SmallMap<K, V> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        DictType::<K, V>::starlark_type_repr()
    }
}

impl<K: StarlarkTypeRepr, V: StarlarkTypeRepr> StarlarkTypeRepr for SmallMap<K, V> {
    type Canonical = <DictType<K, V> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        DictType::<K, V>::starlark_type_repr()
    }
}

// BTreeMap

impl<'v, K: AllocValue<'v>, V: AllocValue<'v>> AllocValue<'v> for BTreeMap<K, V> {
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, K: AllocFrozenValue<'fv>, V: AllocFrozenValue<'fv>> AllocFrozenValue<'fv>
    for BTreeMap<K, V>
{
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<'a, 'v, K: 'a + StarlarkTypeRepr, V: 'a + StarlarkTypeRepr> AllocValue<'v>
    for &'a BTreeMap<K, V>
where
    &'a K: AllocValue<'v>,
    &'a V: AllocValue<'v>,
{
    fn alloc_value(self, heap: Heap<'v>) -> Value<'v> {
        AllocDict(self).alloc_value(heap)
    }
}

impl<'fv, 'a, K: 'a + StarlarkTypeRepr, V: 'a + StarlarkTypeRepr> AllocFrozenValue<'fv>
    for &'a BTreeMap<K, V>
where
    &'a K: AllocFrozenValue<'fv>,
    &'a V: AllocFrozenValue<'fv>,
{
    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> Value<'fv> {
        AllocDict(self).alloc_frozen_value(heap)
    }
}

impl<'a, K: StarlarkTypeRepr, V: StarlarkTypeRepr> StarlarkTypeRepr for &'a BTreeMap<K, V> {
    type Canonical = <BTreeMap<K, V> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        DictType::<K, V>::starlark_type_repr()
    }
}

impl<K: StarlarkTypeRepr, V: StarlarkTypeRepr> StarlarkTypeRepr for BTreeMap<K, V> {
    type Canonical = <DictType<K, V> as StarlarkTypeRepr>::Canonical;

    fn starlark_type_repr() -> Ty {
        DictType::<K, V>::starlark_type_repr()
    }
}
