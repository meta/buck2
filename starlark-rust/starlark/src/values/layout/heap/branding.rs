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

//! An overview of branding in starlark, and the one operation that changes a brand.
//!
//! The lifetimes of starlark values are tied to the lifetimes of a heap that they are allocated in.
//! Starlark uses branded accesses to the heap to ensure that starlark values cannot escape the
//! lifetime of the heap they're tied to, without needing to reference count access to the heap on
//! each starlark value. This module explains how that works.
//!
//! The most important factor in understanding branding is understanding the `'v` lifetime that
//! appears on `Value<'v>`, `Heap<'v>`, and similar types. Unusually, this lifetime should not be
//! understood as representing the "object lifetime" of the heap in a traditional sense. Instead,
//! the `'v` lifetime functions as a unique identifier of a particular heap. In other words, the
//! contract is that if there are two `Value<'v>`s for the same `'v`, the values are allocated in
//! the same heap.
//!
//! To see how this works in practice, it's best to consider an example of how this might fail.
//! Consider code like this:
//!
//! ```rust,ignore
//! # use crate::values::Heap;
//! # use crate::values::Value;
//! # use crate::values::tuple::AllocTuple;
//! Heap::temp(|heap1| {
//!     Heap::temp(|heap2| {
//!         let s1: Value<'_> = heap1.alloc_str("abc").to_value();
//!         let v: Value<'_> = heap2.alloc(AllocTuple([s1]));
//!     })
//! })
//! ```
//!
//! This code attempts to first allocate a string value in `heap1` and then allocate a value in
//! `heap2` that references that value. This is an example of the exact kind of "cross-heap
//! confusion" that we want to disallow, and indeed this fails to compile (though the error message
//! is unfortunately not super helpful):
//!
//! ```text
//! error[E0521]: borrowed data escapes outside of closure
//!   --> fbcode/buck2/starlark-rust/starlark/src/values/layout/heap/branding.rs:55:32
//!    |
//! 52 |     Heap::temp(|heap1| {
//!    |                 -----
//!    |                 |
//!    |                 `heap1` is a reference that is only valid in the closure body
//!    |                 has type `Heap<'1>`
//! ...
//! 55 |             let v: Value<'_> = heap2.alloc(AllocTuple([s1]));
//!    |                                ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//!    |                                |
//!    |                                `heap1` escapes the closure body here
//!    |                                argument requires that `'1` must outlive `'static`
//! ```
//!
//! ### Implementation
//!
//! "Branding" is a searchable term that will yield lots of online discussion about how to make use
//! of this pattern, but in general implementing this behavior consists of only two parts:
//!
//!  1. `Value<'v>`, `Heap<'v>`, and all similar types must be invariant in `'v`. This ensures that
//!     it's never possible to convert a `Value<'v1>` to a `Value<'v2>`.
//!  2. The "root" of all accesses to heaps must be via "branded closures" like `F: for<'v>
//!     FnOnce(Heap<'v>)`, essentially functions with signatures similar to `Heap::temp`.
//!
//! That's it; combined, this means that user code can never prove that the `'v1` coming from one
//! branded closure is the same as the `'v2` coming from another.
//!
//! ### References to frozen heaps
//!
//! To support references to values in frozen heaps, we expand the above contract somewhat. Instead
//! of requiring that a `Value<'v>` must be allocated in a `Heap<'v>`, we require that it must be
//! allocated in a `Heap<'v>` *or in any frozen heap that the `Heap<'v>` depends on.*
//!
//! From the perspective of object lifetimes this is obviously sane (the `Heap<'v>` will keep the
//! dependent frozen heaps alive). We make use of the expanded contract by then providing an API
//! like this:
//!
//! ```rust,ignore
//! impl<T: IsStaticType> OwnedFrozen<T> {
//!     pub fn add_to_heap<'v>(self, heap: Heap<'v>) -> T::Reinfect<'v>;
//! }
//! ```
//!
//! `add_to_heap` adds the heap the frozen value is associated with as a dependency of the passed
//! heap, and then hands the value back at `'v`; the `'v` lifetime in the return value essentially
//! acts as a proof/endorsement that the given value is sound to use "within the context of that
//! heap."
//!
//! ### Heap edges
//!
//! `add_to_heap` is the common case of a more general tool. A `HeapEdge<'v, 'dep>` is a witness
//! that the heap of `'v` keeps the heap of `'dep` alive, and its `rebrand` converts anything at
//! `'dep` to `'v`. Edges are only minted where that dependency is established:
//!
//!  - `OwnedFrozenReconstructor::edge` and `frozen_edge`, inside `by_ref_with_reconstructor`,
//!    add the owner as a reference of the given heap and return the edge to it.
//!  - `Module::frozen_heap` (and `Evaluator::frozen_heap`) opens a scope on a module's own frozen
//!    heap and provides the edge from the module's value heap to it; the dependency is
//!    structural, see `ModuleHeaps`.
//!  - `HeapEdge::immortal` is the edge from every heap to the `'static` brand, at which only
//!    immortal data exists: statics, and the `Methods` tables reached through `&'static Methods`.
//!    `at()` on the static holders is this edge behind a name.
//!  - `HeapEdge::identity` is the edge from a heap to itself, for code written against two brands
//!    that is handed one heap for both.
//!
//! `HeapEdge::unchecked_new` is `unsafe` and crate-private; these minters are its only callers.
//!
//! ### Brands are higher-ranked
//!
//! Every brand at which a heap handle exists (`Heap<'v>`, `FrozenHeap<'fh>`, and the `Module`,
//! `Evaluator`, `Freezer` and deserialize context built on one) is introduced by a closure that
//! is generic over it: `Heap::temp` and `temp_async`, `Module::with_temp_heap` and
//! `with_temp_heap_async`, `OwnedFrozenHeap::with` and `seal_with`, `FrozenHeap::temp`,
//! `OwnedFrozen::build`, `Module::frozen_heap` (with `Evaluator::frozen_heap` and
//! `ModuleHeaps::frozen_heap` behind it), `ModuleHeaps::seal_with`, `GlobalsBuilder::frozen_heap`,
//! and `StarlarkDeserializerImpl::recover_from_pagable`. Everything else that hands out a handle
//! derives it from one it was given (`Module::with_heap`, `Evaluator::new`, `Freezer::new`, the
//! accessors) or from an edge. This is an invariant to keep: never add a constructor of a heap
//! handle from a plain borrow, nothing in the shape of `fn new(&'v OwnedHeap) -> Heap<'v>`.
//!
//! Inside such a closure `'v` is arbitrary (it may as well be `'static`), so a borrow of anything
//! that is not `'static` can never unify with it. A value lent by an owner does not compile:
//!
//! ```compile_fail,E0597
//! use starlark::environment::Module;
//! use starlark::values::FrozenHeapName;
//! use starlark::values::OwnedFrozen;
//! use starlark::values::Value;
//!
//! let owned: OwnedFrozen<Value<'static>> =
//!     OwnedFrozen::build(FrozenHeapName::user("example"), |heap| {
//!         heap.alloc("a value").to_value()
//!     });
//! Module::with_temp_heap(|module| {
//!     module.set("x", owned.as_ref().value());
//! });
//! ```
//!
//! The error says why: "argument requires that `owned` is borrowed for `'static`".
//! `add_to_heap`, which records the dependency, is the way in:
//!
//! ```
//! # use starlark::environment::Module;
//! # use starlark::values::FrozenHeapName;
//! # use starlark::values::OwnedFrozen;
//! # use starlark::values::Value;
//! let owned: OwnedFrozen<Value<'static>> =
//!     OwnedFrozen::build(FrozenHeapName::user("example"), |heap| {
//!         heap.alloc("a value").to_value()
//!     });
//! Module::with_temp_heap(|module| {
//!     module.set("x", owned.as_ref().add_to_heap(module.heap()));
//! });
//! ```
//!
//! Only `'static` owners can lend values at a live brand, and those are immortal. It follows that
//! every frozen `Value<'v>` was minted by an edge (the module's own `HeapEdge<'v, 'fm>`, an
//! `add_to_heap`, `HeapEdge::immortal`) or read out of a value that was.
//!
//! `OwnedFrozenRef<'f, T>` hands values out at the borrow `'f`. That is a brand without a heap
//! handle: nothing can be allocated at it and everything at it is frozen, so no cross-heap
//! pointer can be created there.
//!
//! ### The seal edge
//!
//! A module's optimizer evaluates speculatively at the value heap's `'v` and folds frozen results
//! into IR allocated at the module's frozen heap `'fm`, and the `Freezer` that freezes the module
//! into that heap hands a value that is frozen already back at `'fm` rather than copying it. No
//! `HeapEdge` points that way, but a `SealEdge<'fm, 'v>` does, for frozen values only: it
//! certifies that the frozen heap references every heap in which a frozen `Value<'v>` can live,
//! and its `rebrand` is `OptCtx::demote` and the freezer's fast path. `ModuleHeaps` mints it
//! beside the `HeapEdge`, on a proof in three steps: (a) by the previous section, a frozen
//! `Value<'v>` was minted by the module's own `HeapEdge` (so lives in the frozen heap or in a
//! heap it references), by an `add_to_heap` (which adds the value's heap to the value heap's
//! references first), or from `'static` data, or was read out of such a value, and a value only
//! points into heaps its own heap references; (b) the value heap's references are the frozen
//! heap's at every instant, the two sharing one set; (c) `'static` data is immortal. The one
//! assumption is the contract this module opens with, which every allocator enforces through
//! its brand.
//!
//! The `Freezer<'v, 'fv>` carries the edge, and `Freezer::freeze` takes a `Value<'v>`: a
//! `Freeze<'v>` impl can only hand it values at the brand the edge is for, so a value
//! lent by an unrelated owner does not compile (the higher-ranked brand rules it out, as above).
//!
//! ### What is trusted rather than proven
//!
//! Nothing beyond the contract this module opens with. Every brand change goes through an edge,
//! and every edge is minted where the dependency it certifies is established. The `'static`
//! brand is honest: apart from the private erased storage of the owning carriers
//! (`OwnedFrozen`, `FrozenModule`, `Globals`) and of the heaps themselves (their arenas and string
//! interners), which is filled at a brand and only ever read back at a brand the owner vouches
//! for, the only data at `'static` is immortal.

use crate::any::ProvidesStaticType;
use crate::cast::transmute;

/// `v`, which is at the `'from` brand, at the `'to` brand.
///
/// This is the one place a brand changes; [`HeapEdge::rebrand`](super::edge::HeapEdge::rebrand)
/// and the accessors of `OwnedFrozen` are calls of it with the obligation below discharged.
/// `From` and `To` are one type at two brands: two types with the same `StaticType` differ only
/// in lifetimes, by the contract of [`ProvidesStaticType`], so no bytes change and the question
/// is a lifetime one.
///
/// # Safety
///
/// Everything `v` refers to must be kept alive by the heap identified by `'to`, so that it is
/// usable wherever a value at `'to` is. `'to` may be `'static` only for the private erased storage
/// of an owner, which reads it back at a brand the owner vouches for, see above.
pub(in crate::values::layout::heap) unsafe fn rebrand_unchecked<'from, 'to, S, From, To>(
    v: From,
) -> To
where
    S: ?Sized,
    From: ProvidesStaticType<'from, StaticType = S>,
    To: ProvidesStaticType<'to, StaticType = S>,
{
    // SAFETY: Same layout by the bounds, see above; the lifetime is the caller's obligation.
    unsafe { transmute!(From, To, v) }
}

/// [`rebrand_unchecked`], behind a reference: the borrow is kept.
///
/// # Safety
///
/// As for [`rebrand_unchecked`].
pub(in crate::values::layout::heap) unsafe fn rebrand_ref_unchecked<'a, 'from, 'to, S, From, To>(
    v: &'a From,
) -> &'a To
where
    S: ?Sized,
    From: ProvidesStaticType<'from, StaticType = S>,
    To: ProvidesStaticType<'to, StaticType = S>,
{
    // SAFETY: References to two types of one layout have one layout; see `rebrand_unchecked`.
    unsafe { transmute!(&'a From, &'a To, v) }
}
