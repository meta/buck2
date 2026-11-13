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

use crate::values::AllocFrozenValue;
use crate::values::AllocValue;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::Value;
use crate::values::ValueOf;
use crate::values::list::ListType;
use crate::values::type_expr::StarlarkTypeExpr;

/// Utility to allocate a list from iterator.
///
/// # Example
///
/// ```
/// use starlark::values::list::AllocList;
///
/// # use starlark::values::{FrozenHeap, Heap};
/// # fn alloc(heap: Heap<'_>, frozen_heap: FrozenHeap<'_>) {
/// let l = heap.alloc(AllocList([1, 2, 3]));
/// let ls = frozen_heap.alloc(AllocList([1, 2, 3]));
/// # }
/// ```
pub struct AllocList<L>(pub L);

impl<'v> AllocList<iter::Empty<Value<'v>>> {
    /// A new empty list whose items are `typing.Any`, for where the result is used untyped
    /// (`heap.alloc(AllocList::EMPTY).to_value()`). [`empty`](AllocList::empty) is the one for
    /// a typed field or return.
    pub const EMPTY: AllocList<iter::Empty<Value<'v>>> = AllocList(iter::empty());
}

impl<'v, T: StarlarkTypeExpr<'v>> AllocList<iter::Empty<ValueOf<'v, T>>> {
    /// A new empty list of the item type the context asks for:
    ///
    /// ```
    /// use starlark::values::Heap;
    /// use starlark::values::ValueOf;
    /// use starlark::values::list::AllocList;
    /// use starlark::values::list::ListType;
    /// use starlark::values::string::StarlarkStr;
    ///
    /// Heap::temp(|heap| {
    ///     let names: ValueOf<ListType<StarlarkStr>> = heap.alloc(AllocList::empty());
    ///     assert!(names.list().is_empty());
    /// });
    /// ```
    ///
    /// Where nothing asks for an item type, use [`EMPTY`](AllocList::EMPTY). Both allocate; the
    /// immutable empty list that needs no heap is [`ValueOf::empty_list`].
    pub fn empty() -> Self {
        AllocList(iter::empty())
    }
}

impl<'v, L> AllocValue<'v> for AllocList<L>
where
    L: IntoIterator,
    L::Item: AllocValue<'v>,
{
    type Type = ListType<<L::Item as AllocValue<'v>>::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        ValueOf::alloc_list(heap, self.0.into_iter().map(|x| x.alloc_value(heap)))
    }
}

impl<'fv, L> AllocFrozenValue<'fv> for AllocList<L>
where
    L: IntoIterator,
    L::Item: AllocFrozenValue<'fv>,
{
    type Type = ListType<<L::Item as AllocFrozenValue<'fv>>::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        // The items are `L::Item::Type` by `L::Item`'s declaration.
        ValueOf::new_unchecked(
            heap.alloc_list_iter(self.0.into_iter().map(|x| heap.alloc(x).to_value())),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::values::FrozenHeap;
    use crate::values::Heap;
    use crate::values::ValueOf;
    use crate::values::list::AllocList;
    use crate::values::list::ListType;
    use crate::values::string::StarlarkStr;

    #[test]
    fn test_empty_takes_its_item_type_from_the_context() {
        fn names<'v>(heap: Heap<'v>) -> ValueOf<'v, ListType<StarlarkStr>> {
            heap.alloc(AllocList::empty())
        }
        Heap::temp(|heap| {
            let list = names(heap);
            assert!(list.list().is_empty());
            // A new list each time, not the immutable constant.
            assert!(!list.to_value().is_frozen());
            assert!(!list.to_value().ptr_eq(names(heap).to_value()));
        });
        FrozenHeap::temp(|heap| {
            let list: ValueOf<ListType<StarlarkStr>> = heap.alloc(AllocList::empty());
            assert!(list.list().is_empty());
        });
    }
}
