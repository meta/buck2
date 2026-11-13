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

use std::iter;

use starlark_map::small_map::SmallMap;

use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::dict::Dict;
use crate::values::type_expr::StarlarkTypeExpr;
use crate::values::types::dict::dict_type::DictType;

/// Utility to allocate a dict from iterator.
///
/// Iterator must be a list of pairs (key, value).
/// Duplicate keys are allowed, last key wins.
///
/// # Panics
///
/// Panics if a key is not hashable.
///
/// # Example
///
/// ```
/// use starlark::values::dict::AllocDict;
///
/// # use starlark::values::{FrozenHeap, Heap};
/// # fn alloc(heap: Heap<'_>, frozen_heap: FrozenHeap<'_>) {
/// let l = heap.alloc(AllocDict([("a", 1), ("b", 2), ("c", 3)]));
/// let ls = frozen_heap.alloc(AllocDict([("a", 1), ("b", 2), ("c", 3)]));
/// # }
/// ```
pub struct AllocDict<D>(pub D);

impl<'v> AllocDict<iter::Empty<(Value<'v>, Value<'v>)>> {
    /// A new empty dict whose keys and values are `typing.Any`, for where the result is used
    /// untyped (`heap.alloc(AllocDict::EMPTY).to_value()`). [`empty`](AllocDict::empty) is the
    /// one for a typed field or return.
    pub const EMPTY: AllocDict<iter::Empty<(Value<'v>, Value<'v>)>> = AllocDict(iter::empty());
}

impl<'v, K: StarlarkTypeExpr<'v>, V: StarlarkTypeExpr<'v>>
    AllocDict<iter::Empty<(ValueOf<'v, K>, ValueOf<'v, V>)>>
{
    /// A new empty dict of the key and value types the context asks for:
    ///
    /// ```
    /// use starlark::values::Heap;
    /// use starlark::values::ValueOf;
    /// use starlark::values::dict::AllocDict;
    /// use starlark::values::dict::DictType;
    /// use starlark::values::string::StarlarkStr;
    ///
    /// Heap::temp(|heap| {
    ///     let env: ValueOf<DictType<StarlarkStr, StarlarkStr>> = heap.alloc(AllocDict::empty());
    ///     assert!(env.dict().is_empty());
    /// });
    /// ```
    ///
    /// Where nothing asks for the types, use [`EMPTY`](AllocDict::EMPTY). Both allocate; the
    /// immutable empty dict that needs no heap is [`ValueOf::empty_dict`].
    pub fn empty() -> Self {
        AllocDict(iter::empty())
    }
}

impl<'v, D, K, V> AllocValue<'v> for AllocDict<D>
where
    D: IntoIterator<Item = (K, V)>,
    K: AllocValue<'v>,
    V: AllocValue<'v>,
{
    type Type = DictType<K::Type, V::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc_dict(
            heap,
            self.0
                .into_iter()
                .map(|(k, v)| (k.alloc_value(heap), v.alloc_value(heap))),
        )
    }
}

impl<'fv, D, K, V> AllocFrozenValue<'fv> for AllocDict<D>
where
    D: IntoIterator<Item = (K, V)>,
    K: AllocFrozenValue<'fv>,
    V: AllocFrozenValue<'fv>,
{
    type Type = DictType<K::Type, V::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        let iter = self.0.into_iter();
        let mut map = SmallMap::with_capacity(iter.size_hint().0);
        for (k, v) in iter {
            map.insert_hashed(
                k.alloc_frozen_value(heap).to_value().get_hashed().unwrap(),
                v.alloc_frozen_value(heap).to_value(),
            );
        }
        // The entries are `K::Type` and `V::Type` by their declarations.
        ValueOf::new_unchecked(heap.alloc(Dict::new(map)).to_value())
    }
}

#[cfg(test)]
mod tests {
    use crate::values::FrozenHeap;
    use crate::values::Heap;
    use crate::values::ValueOf;
    use crate::values::dict::AllocDict;
    use crate::values::dict::DictType;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_empty_takes_its_types_from_the_context() {
        fn env<'v>(heap: Heap<'v>) -> ValueOf<'v, DictType<StarlarkStr, StarlarkStr>> {
            heap.alloc(AllocDict::empty())
        }
        Heap::temp(|heap| {
            let dict = env(heap);
            assert!(dict.dict().is_empty());
            // A new dict each time, not the immutable constant.
            assert!(!dict.to_value().is_frozen());
            assert!(!dict.to_value().ptr_eq(env(heap).to_value()));
        });
        FrozenHeap::temp(|heap| {
            let dict: ValueOf<DictType<StarlarkStr, StarlarkStr>> = heap.alloc(AllocDict::empty());
            assert!(dict.dict().is_empty());
        });
    }
}
