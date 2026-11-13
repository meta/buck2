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
use crate::values::tuple::TupleType;

/// Utility to allocate a tuple.
///
/// Note, for tuples of fixed sizes there are implementations for `(A,)` or `(A, B)`.
///
/// # Example
///
/// ```
/// # use starlark::values::{FrozenHeap, Heap};
/// # use starlark::values::tuple::AllocTuple;
/// # fn alloc(heap: Heap<'_>, frozen_heap: FrozenHeap<'_>) {
/// let l = heap.alloc(AllocTuple([1, 2, 3]));
/// let ls = frozen_heap.alloc(AllocTuple([1, 2, 3]));
/// # }
/// ```
pub struct AllocTuple<T>(pub T);

impl<'v> AllocTuple<iter::Empty<Value<'v>>> {
    /// Allocate an empty tuple.
    pub const EMPTY: AllocTuple<iter::Empty<Value<'v>>> = AllocTuple(iter::empty());
}

impl<'v, T> AllocValue<'v> for AllocTuple<T>
where
    T: IntoIterator,
    T::Item: AllocValue<'v>,
{
    type Type = TupleType<<T::Item as AllocValue<'v>>::Type>;

    fn alloc_value(self, heap: Heap<'v>) -> ValueOf<'v, Self::Type> {
        // The items are `T::Item::Type` by `T::Item`'s declaration.
        ValueOf::new_unchecked(
            heap.alloc_tuple_iter(self.0.into_iter().map(|x| heap.alloc(x).to_value())),
        )
    }
}

impl<'fv, T> AllocFrozenValue<'fv> for AllocTuple<T>
where
    T: IntoIterator,
    T::Item: AllocFrozenValue<'fv>,
{
    type Type = TupleType<<T::Item as AllocFrozenValue<'fv>>::Type>;

    fn alloc_frozen_value(self, heap: FrozenHeap<'fv>) -> ValueOf<'fv, Self::Type> {
        // The items are `T::Item::Type` by `T::Item`'s declaration.
        ValueOf::new_unchecked(
            heap.alloc_tuple_iter(
                self.0
                    .into_iter()
                    .map(|x| x.alloc_frozen_value(heap).to_value()),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::values::FrozenHeap;
    use crate::values::Heap;
    use crate::values::tuple::TupleRef;
    use crate::values::tuple::alloc::AllocTuple;

    #[test]
    fn test_alloc_tuple() {
        Heap::temp(|heap| {
            let a = heap.alloc(AllocTuple([""; 0])).to_value();
            let b = heap
                .alloc(AllocTuple([1, 2, 3].iter().copied().filter(|_| false)))
                .to_value();
            assert_eq!(0, TupleRef::from_value(a).unwrap().content().len());
            assert!(a.ptr_eq(b));

            // Fixed length iterator.
            let c = heap.alloc(AllocTuple([1, 2])).to_value();
            assert_eq!(2, TupleRef::from_value(c).unwrap().content().len());

            // Iterator of unknown length.
            let d = heap
                .alloc(AllocTuple([1, 2, 3].iter().copied().filter(|c| *c > 1)))
                .to_value();
            assert_eq!(2, TupleRef::from_value(d).unwrap().content().len());
        });
    }

    #[test]
    fn test_alloc_frozen_tuple() {
        FrozenHeap::temp(|heap| {
            let a = heap.alloc(AllocTuple([""; 0])).to_value();
            let b = heap
                .alloc(AllocTuple([1, 2, 3].iter().copied().filter(|_| false)))
                .to_value();
            assert_eq!(0, TupleRef::from_value(a).unwrap().content().len());
            assert!(a.ptr_eq(b));

            // Fixed length iterator.
            let c = heap.alloc(AllocTuple([1, 2])).to_value();
            assert_eq!(2, TupleRef::from_value(c).unwrap().content().len());

            // Iterator of unknown length.
            let d = heap
                .alloc(AllocTuple([1, 2, 3].iter().copied().filter(|c| *c > 1)))
                .to_value();
            assert_eq!(2, TupleRef::from_value(d).unwrap().content().len());
        });
    }
}
