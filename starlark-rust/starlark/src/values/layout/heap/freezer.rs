/*
 * Copyright 2019 The Starlark in Rust Authors.
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

#[cfg(debug_assertions)]
use std::cell::Cell;
use std::cell::RefCell;

use crate::eval::compiler::def::Def;
use crate::values::AllocFrozenValue;
use crate::values::FreezeResult;
use crate::values::FrozenHeap;
use crate::values::SealEdge;
use crate::values::ValueOf;
use crate::values::ValueTyped;
use crate::values::layout::heap::repr::AValueHeapEntryState;
use crate::values::layout::value::Value;

/// Copies values from the heap of `'v` onto the frozen heap of `'fv`; the argument of
/// [`Freeze::freeze`](crate::values::Freeze::freeze).
///
/// [`freeze`](Freezer::freeze) copies an unfrozen value onto the freezer's heap through the
/// value's `Freeze` impl and overwrites the original with a forwarding pointer to the
/// copy, so a value reachable from several places is copied once and keeps its identity. The
/// originals are unusable from then on, which is why freezing consumes the
/// [`Module`](crate::environment::Module).
///
/// A value that is already frozen is not copied: [`freeze`](Freezer::freeze) hands it back at
/// `'fv` through the `SealEdge` the freezer holds, which certifies that the target heap
/// references every heap in which a frozen `Value<'v>` can live. `ModuleHeaps::seal_with` mints
/// that edge along with the freezer, so a freezer only ever exists for a module's own two heaps.
pub struct Freezer<'v, 'fv> {
    /// Freezing into this heap.
    pub(crate) heap: FrozenHeap<'fv>,
    /// From the heap being frozen to `heap`, for the values that are frozen already.
    seal_edge: SealEdge<'fv, 'v>,
    /// Defs frozen by this freezer.
    pub(crate) frozen_defs: RefCell<Vec<ValueTyped<'fv, Def<'fv>>>>,
    /// A freeze error occurred; the freezer must be abandoned.
    #[cfg(debug_assertions)]
    failed: Cell<bool>,
}

impl<'v, 'fv> Freezer<'v, 'fv> {
    pub(in crate::values::layout::heap) fn new(
        heap: FrozenHeap<'fv>,
        seal_edge: SealEdge<'fv, 'v>,
    ) -> Self {
        Freezer {
            heap,
            seal_edge,
            frozen_defs: RefCell::new(Vec::new()),
            #[cfg(debug_assertions)]
            failed: Cell::new(false),
        }
    }

    /// Allocate a new value while freezing. Usually not a great idea.
    pub fn alloc<T: AllocFrozenValue<'fv>>(&self, val: T) -> ValueOf<'fv, T::Type> {
        self.heap.alloc(val)
    }

    /// Freeze a nested value while freezing yourself.
    pub fn freeze(&self, value: Value<'v>) -> FreezeResult<Value<'fv>> {
        // An error leaves sources forwarding to unpublished reservations, so
        // the whole freeze must be abandoned.
        #[cfg(debug_assertions)]
        assert!(
            !self.failed.get(),
            "freezing must not continue after an earlier freeze error"
        );

        // Case 1: Already frozen, so nothing to copy.
        if let Some(frozen) = self.seal_edge.rebrand(value) {
            return Ok(frozen);
        }

        // Case 2: We have already been replaced with a forwarding, or need to freeze
        let entry = value.0.unpack_ptr().unwrap();
        match entry.state() {
            AValueHeapEntryState::Forward(x) => {
                Ok(unsafe { x.forward_ptr().unpack_frozen_value() })
            }
            AValueHeapEntryState::Value(_) => {
                // SAFETY: just checked to be a live value.
                let v = unsafe { value.0.value_header_ptr_unchecked() };
                let result = unsafe { v.unpack().heap_freeze(self) };
                #[cfg(debug_assertions)]
                if result.is_err() {
                    self.failed.set(true);
                }
                result
            }
            AValueHeapEntryState::Reservation(_) => {
                unreachable!("cannot freeze a heap reservation")
            }
        }
    }

    /// Frozen heap where the values are frozen to.
    ///
    /// Can be used to allocate additional values while freezing.
    pub fn frozen_heap(&self) -> FrozenHeap<'fv> {
        self.heap
    }
}

#[cfg(test)]
impl Freezer<'_, '_> {
    /// Run `f` with a value heap and a freezer out of it, both scoped to the call. The heaps are
    /// a `ModuleHeaps`, which is where the freezer's [`SealEdge`] comes from.
    pub(crate) fn testing_temp<R>(
        f: impl for<'v, 'fv> FnOnce(crate::values::Heap<'v>, &Freezer<'v, 'fv>) -> R,
    ) -> R {
        crate::values::Heap::temp(|heap| {
            crate::values::layout::heap::module_heaps::ModuleHeaps::new(heap)
                .frozen_heap(|fh, _edge, seal_edge| f(heap, &Freezer::new(fh, seal_edge)))
        })
    }
}
