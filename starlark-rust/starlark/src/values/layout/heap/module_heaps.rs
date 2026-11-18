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

use crate::any::IsStaticType;
use crate::values::Freezer;
use crate::values::FrozenHeap;
use crate::values::Heap;
use crate::values::HeapEdge;
use crate::values::HeapSendable;
use crate::values::HeapSyncable;
use crate::values::OwnedFrozen;
use crate::values::OwnedFrozenHeap;
use crate::values::SealEdge;
use crate::values::layout::heap::name::FrozenHeapName;

/// The two heaps of a [`Module`](crate::environment::Module): the value heap and the frozen heap
/// it is building, owned together.
///
/// Values allocated on the frozen heap are handed out at the value heap's brand, through the
/// [`HeapEdge`] that [`frozen_heap`](ModuleHeaps::frozen_heap) provides. That is sound because the
/// builder never leaves this type unsealed: [`seal_with`](ModuleHeaps::seal_with) and [`Drop`] are its
/// only exits, and both seal it into the value heap's references. `seal_with` does so whether its
/// closure returns `Ok` or `Err`, and `Drop` covers a module that is never frozen as well as a
/// closure that unwinds (skipping a builder nothing was allocated on, which has nothing at `'fm`
/// to keep alive). The edge therefore exists from the moment the two heaps do, with nothing to
/// remember at the end.
///
/// The other direction is covered too: the two heaps hold one set of references between them. A
/// heap that either of them comes to depend on (the value heap through a `load` or an
/// `add_to_heap`, the builder through the globals' edge) is a dependency of both from that moment
/// on, so the builder, and the sealed heap it becomes, keeps alive every heap that a frozen value
/// at `'v` can have been copied out of, and does so from the moment the value heap references
/// it rather than from sealing.
#[derive(Debug)]
pub(crate) struct ModuleHeaps<'v> {
    heap: Heap<'v>,
    /// `None` only after `seal_with` has taken the builder out; `Drop` then has nothing to do.
    frozen: Option<OwnedFrozenHeap>,
}

impl<'v> ModuleHeaps<'v> {
    pub(crate) fn new(heap: Heap<'v>) -> Self {
        Self {
            heap,
            frozen: Some(OwnedFrozenHeap::sharing_references(heap.references())),
        }
    }

    pub(crate) fn heap(&self) -> Heap<'v> {
        self.heap
    }

    /// Allocate on the frozen heap, see [`Module::frozen_heap`](crate::environment::Module::frozen_heap);
    /// `f` also gets the [`SealEdge`] from the value heap back to the frozen heap.
    pub(crate) fn frozen_heap<R>(
        &self,
        f: impl for<'fm> FnOnce(FrozenHeap<'fm>, HeapEdge<'v, 'fm>, SealEdge<'fm, 'v>) -> R,
    ) -> R {
        self.frozen().with(|fh| {
            let (edge, seal_edge) = self.edges(fh);
            f(fh, edge, seal_edge)
        })
    }

    /// The edges between the value heap and the builder, for a handle that `with` opened on the
    /// builder: the [`HeapEdge`] that brings the builder's allocations to `'v`, and the
    /// [`SealEdge`] that brings frozen values at `'v` back to the builder's brand.
    fn edges<'fm>(&self, _fh: FrozenHeap<'fm>) -> (HeapEdge<'v, 'fm>, SealEdge<'fm, 'v>) {
        // For both: `'fm` is introduced by the `with` closure, so it is a true brand, and `'v` is
        // a brand by the assumption every use of it makes.
        //
        // SAFETY: This type is the only owner of the builder, and the builder only leaves it
        // through `seal_with` or `Drop`, both of which seal it into `heap`'s references unless
        // nothing was allocated on it. So `'fm`'s allocations live as long as the value heap,
        // which is as long as anything at `'v` can.
        let edge = unsafe { HeapEdge::unchecked_new() };
        // SAFETY: A frozen `Value<'v>` lives in a heap the builder references, or is immortal:
        //
        //  (a) Brands are introduced only by higher-ranked closures (`Heap::temp` and its kin;
        //      the `branding` module states the invariant), so within its scope `'v` is
        //      arbitrary and no borrow of a non-`'static` owner unifies with it. A frozen
        //      `Value<'v>` is therefore minted by one of: `edge` above, from a value in the
        //      builder or in a heap the builder references; `Heap::add_reference` followed by a
        //      rebrand (`add_to_heap`, `OwnedFrozenReconstructor::edge`), which puts the value's
        //      heap into `heap`'s references first; or `HeapEdge::immortal`, from `'static`
        //      data. Or it is read out of another value at `'v`, which by induction lives in the
        //      value heap or in one of those heaps, and a value only points into its own heap or
        //      into heaps its heap references (the contract on `Value<'v>` that the `branding`
        //      module opens with, which every allocator enforces through its brand).
        //  (b) `heap`'s references are the builder's: the two hold one set (see `new`), so a
        //      heap that `add_reference` adds is referenced by the builder from that instant,
        //      before the value that made it necessary exists at `'v`.
        //  (c) `'static` data is immortal, which every heap trivially keeps alive.
        let seal_edge = unsafe { SealEdge::unchecked_new() };
        (edge, seal_edge)
    }

    pub(crate) fn frozen_heap_allocated_bytes(&self) -> usize {
        self.frozen().allocated_bytes()
    }

    fn frozen(&self) -> &OwnedFrozenHeap {
        self.frozen
            .as_ref()
            .expect("the builder is only taken by `seal_with`, which consumes `self`")
    }

    /// Freeze the frozen heap's root value with `f`, then seal the heap into the value heap's
    /// references and return the root value kept alive by it. `f` gets a [`Freezer`] from the
    /// value heap into the frozen heap, and the same edges as
    /// [`frozen_heap`](ModuleHeaps::frozen_heap).
    ///
    /// The heap is sealed whether `f` succeeds, fails or panics. The `name` is the sealed heap's,
    /// see [`OwnedFrozen::name`].
    pub(crate) fn seal_with<T, E>(
        mut self,
        name: Option<FrozenHeapName>,
        f: impl for<'fm> FnOnce(
            &Freezer<'v, 'fm>,
            HeapEdge<'v, 'fm>,
            SealEdge<'fm, 'v>,
        ) -> Result<T::Reinfect<'fm>, E>,
    ) -> Result<OwnedFrozen<T>, E>
    where
        T: IsStaticType,
        for<'fv> T::Reinfect<'fv>: HeapSendable<'fv> + HeapSyncable<'fv> + Sized,
    {
        // The builder stays in `self` while `f` runs, so that if `f` unwinds, `Drop` seals it like
        // on every other exit: the value heap may already hold pointers into it.
        //
        // SAFETY: `'fm` is the brand of the builder, which is sealed right below into the owner
        // the value is paired with. Being closure-introduced, `'fm` names nothing else.
        let root = self.frozen().with(|fh| {
            let (edge, seal_edge) = self.edges(fh);
            let freezer = Freezer::new(fh, seal_edge);
            f(&freezer, edge, seal_edge).map(|v| unsafe { OwnedFrozen::<T>::erase_brand(v) })
        });
        let frozen = self
            .frozen
            .take()
            .expect("the builder is only taken by `seal_with`, which consumes `self`");
        let sealed = frozen.seal_impl(name, Some(self.heap.peak_allocated_bytes()));
        self.heap.add_reference(sealed.owner());
        // SAFETY: `sealed` is the heap that `'fm` named.
        root.map(|v| unsafe { OwnedFrozen::from_erased(sealed, v) })
    }
}

impl<'v> Drop for ModuleHeaps<'v> {
    fn drop(&mut self) {
        // A module that is dropped rather than frozen, or a `seal_with` closure that unwound: the
        // frozen heap has to outlive the module all the same, see the type doc. A builder with no
        // allocations would seal into a heap that keeps alive only what the value heap already
        // does, so it is skipped.
        if let Some(frozen) = self.frozen.take()
            && frozen.has_allocations()
        {
            let sealed = frozen.seal_impl(None, Some(self.heap.peak_allocated_bytes()));
            self.heap.add_reference(sealed.owner());
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::values::Heap;
    use crate::values::OwnedFrozen;
    use crate::values::Value;
    use crate::values::layout::heap::module_heaps::ModuleHeaps;
    use crate::values::layout::heap::name::StarlarkTestHeapName;

    /// A frozen value from a heap that only the value heap references is frozen by pointer copy,
    /// so the sealed heap has to reference that heap too, even when the reference was added
    /// while sealing.
    #[test]
    fn test_seal_with_inherits_references_added_while_sealing() {
        let expected = "a string that lives on a heap of its own".repeat(8);
        let foreign =
            OwnedFrozen::<Value<'static>>::build(StarlarkTestHeapName::frozen_heap_name(), |fh| {
                fh.alloc(expected.as_str()).to_value()
            });
        let root = Heap::temp(|heap| {
            let heaps = ModuleHeaps::new(heap);
            heaps
                .seal_with::<Value<'static>, ()>(None, |freezer, _edge, _seal_edge| {
                    let v = foreign.as_ref().add_to_heap(heap);
                    Ok(freezer.freeze(v).unwrap())
                })
                .unwrap()
        });
        assert!(root.refs().any(|r| r == foreign.owner()));
        root.by_ref(|v| assert_eq!(v.unpack_str(), Some(expected.as_str())));
    }

    /// The value heap's references are the builder's at the moment they are added, not at
    /// sealing: the optimizer demotes values from heaps the value heap references into the
    /// builder while the module is still being compiled. And the builder's are the value heap's.
    #[test]
    fn test_the_two_heaps_share_their_references() {
        let foreign =
            OwnedFrozen::<Value<'static>>::build(StarlarkTestHeapName::frozen_heap_name(), |fh| {
                fh.alloc(
                    "a string that lives on a heap of its own"
                        .repeat(8)
                        .as_str(),
                )
                .to_value()
            });
        let other =
            OwnedFrozen::<Value<'static>>::build(StarlarkTestHeapName::frozen_heap_name(), |fh| {
                fh.alloc(
                    "a string that lives on another heap of its own"
                        .repeat(8)
                        .as_str(),
                )
                .to_value()
            });
        Heap::temp(|heap| {
            let heaps = ModuleHeaps::new(heap);
            heaps.frozen_heap(|fh, _edge, _seal_edge| {
                assert!(
                    !fh.referenced_heaps()
                        .iter()
                        .any(|r| r.owner() == foreign.owner())
                );
                foreign.as_ref().add_to_heap(heap);
                assert!(
                    fh.referenced_heaps()
                        .iter()
                        .any(|r| r.owner() == foreign.owner())
                );
                fh.add_reference(other.owner());
            });
            let referenced = heap.referenced_heaps();
            assert!(referenced.iter().any(|r| r.owner() == foreign.owner()));
            assert!(referenced.iter().any(|r| r.owner() == other.owner()));
        });
    }

    /// The seal edge brings frozen values at `'v` to the builder's brand, whichever way they got
    /// to `'v`, and refuses unfrozen ones.
    #[test]
    fn test_seal_edge_rebrands_frozen_values_only() {
        let foreign =
            OwnedFrozen::<Value<'static>>::build(StarlarkTestHeapName::frozen_heap_name(), |fh| {
                fh.alloc(
                    "a string that lives on a heap of its own"
                        .repeat(8)
                        .as_str(),
                )
                .to_value()
            });
        Heap::temp(|heap| {
            let heaps = ModuleHeaps::new(heap);
            heaps.frozen_heap(|fh, edge, seal_edge| {
                let from_builder =
                    edge.rebrand(fh.alloc("allocated on the builder".repeat(8)).to_value());
                let from_owner = foreign.as_ref().add_to_heap(heap);
                let unfrozen = heap
                    .alloc("allocated on the value heap".repeat(8))
                    .to_value();
                assert!(
                    seal_edge
                        .rebrand(from_builder)
                        .unwrap()
                        .ptr_eq(from_builder)
                );
                assert!(seal_edge.rebrand(from_owner).unwrap().ptr_eq(from_owner));
                assert!(
                    seal_edge
                        .rebrand(Value::new_none())
                        .unwrap()
                        .ptr_eq(Value::new_none())
                );
                assert!(seal_edge.rebrand(unfrozen).is_none());
            });
        });
    }
}

// `catch_unwind` needs an unwinding panic runtime; under `panic = "abort"` the hole this guards
// against cannot be exercised, since a panic ends the process.
#[cfg(all(test, panic = "unwind"))]
mod unwind_tests {
    use std::panic::AssertUnwindSafe;
    use std::panic::catch_unwind;

    use crate::values::Heap;
    use crate::values::Value;
    use crate::values::layout::heap::module_heaps::ModuleHeaps;
    use crate::values::list::AllocList;
    use crate::values::list::ListRef;

    /// The value heap can hold pointers into the frozen heap before it is sealed, so a panic
    /// inside the sealing closure must still seal it.
    #[test]
    fn test_unwinding_out_of_seal_with_seals() {
        Heap::temp(|heap| {
            let heaps = ModuleHeaps::new(heap);
            let expected = "a string that lives on the module's frozen heap".repeat(8);
            let s =
                heaps.frozen_heap(|fh, edge, _seal_edge| edge.rebrand(fh.alloc(expected.as_str())));
            let list = heap.alloc(AllocList(vec![s])).to_value();
            let references_before = heap.referenced_heaps().len();
            let unwound = catch_unwind(AssertUnwindSafe(|| {
                heaps.seal_with::<Value<'static>, ()>(None, |_freezer, _edge, _seal_edge| {
                    panic!("sealing failed")
                })
            }));
            assert!(unwound.is_err());
            // Reading the string back would not reliably catch a use after free, so check that
            // the builder was sealed into the value heap's references.
            assert_eq!(heap.referenced_heaps().len(), references_before + 1);
            let list = ListRef::from_value(list).unwrap();
            assert_eq!(list.content()[0].unpack_str(), Some(expected.as_str()));
        })
    }
}
