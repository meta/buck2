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

//! Starlark heap implementation.
//!
//! Values live in heaps, and a value is only usable while its heap is alive. Rather than
//! reference counting every value, Starlark tracks heaps, and the lifetime parameter of
//! `Value<'v>` ties each value to the heap it lives in: two values with the same brand live in the
//! same heap, or in heaps that heap keeps alive. The `branding` module explains that discipline
//! and where brands come from; this one describes the heaps and how values move between them.
//!
//! # The heaps
//!
//! * [`Heap<'v>`] is an unfrozen heap, the one a module's program allocates on. Values on it may
//!   be mutable and are garbage collected.
//! * [`OwnedFrozenHeap`] is a frozen heap under construction: owned, single threaded, and
//!   allocated on through the [`FrozenHeap<'fh>`] handle it hands out inside a closure. Values
//!   allocated through the handle are immutable and branded `'fh`, so they cannot leave the
//!   closure on their own.
//! * [`OwnedFrozen<()>`] is a sealed frozen heap: read only, `Clone`, `Send + Sync`, and the thing
//!   that keeps a frozen heap's memory alive. [`OwnedFrozenHeap::seal`] produces one. An
//!   `OwnedFrozen<T>` is a sealed heap together with a `T` that it keeps alive, and
//!   [`OwnedFrozenRef`] is its borrowed form; the type docs list the ways of reaching the `T`.
//!
//! # Heap references
//!
//! A heap can depend on sealed frozen heaps: [`Heap::add_reference`] and
//! [`FrozenHeap::add_reference`] keep the given heap alive for as long as the referencing heap
//! is. The invariant everything relies on is that a value in heap A may point at a value in heap B
//! only if A is B, or A references B, directly or through other heaps. The safe APIs maintain it
//! without the caller thinking about it: every way of bringing a value out of a sealed heap to
//! the brand of another heap adds the reference as a side effect ([`OwnedFrozen::add_to_heap`],
//! [`OwnedFrozenReconstructor::edge`]), and a [`HeapEdge`] is the witness of a reference that
//! exists. The `branding` module lists where edges are minted, and the one brand change that rests
//! on a contract rather than an edge.
//!
//! # Where Starlark itself adds references
//!
//! * A [`Module`] owns a `Heap<'v>` and an `OwnedFrozenHeap` together, as a [`ModuleHeaps`]. The
//!   frozen heap holds the compiler's products, the two share one set of references, and the
//!   frozen heap is sealed into the value heap's references whether the module is frozen or
//!   dropped.
//! * Compiling a module adds the [`Globals`] heap as a reference of the module's frozen heap, so
//!   that the compiled code can refer to the globals' values directly.
//! * `load()` adds the loaded [`FrozenModule`]'s heap as a reference of the loading module's
//!   frozen heap.
//! * Freezing seals the module's frozen heap, with every reference the module gained, into the
//!   `FrozenModule`.
//! * [`Globals`], [`Methods`] and [`FrozenModule`] own the sealed heaps their values live in.
//!   Statics ([`AllocStaticSimple`], [`const_frozen_string!`]) are in no heap and need no
//!   reference, see [`HeapEdge::immortal`].
//!
//! [`Heap<'v>`]: crate::values::Heap
//! [`Heap::add_reference`]: crate::values::Heap::add_reference
//! [`FrozenHeap<'fh>`]: crate::values::FrozenHeap
//! [`FrozenHeap::add_reference`]: crate::values::FrozenHeap::add_reference
//! [`OwnedFrozenHeap`]: crate::values::OwnedFrozenHeap
//! [`OwnedFrozenHeap::seal`]: crate::values::OwnedFrozenHeap::seal
//! [`OwnedFrozen<()>`]: crate::values::OwnedFrozen
//! [`OwnedFrozen::add_to_heap`]: crate::values::OwnedFrozen::add_to_heap
//! [`OwnedFrozenRef`]: crate::values::OwnedFrozenRef
//! [`OwnedFrozenReconstructor::edge`]: owned_frozen::OwnedFrozenReconstructor::edge
//! [`HeapEdge`]: crate::values::HeapEdge
//! [`HeapEdge::immortal`]: crate::values::HeapEdge::immortal
//! [`ModuleHeaps`]: module_heaps::ModuleHeaps
//! [`Module`]: crate::environment::Module
//! [`FrozenModule`]: crate::environment::FrozenModule
//! [`Globals`]: crate::environment::Globals
//! [`Methods`]: crate::environment::Methods
//! [`AllocStaticSimple`]: crate::values::AllocStaticSimple
//! [`const_frozen_string!`]: crate::const_frozen_string

pub(crate) mod allocator;
pub(crate) mod arena;
mod branding;
pub(crate) mod call_enter_exit;
pub(crate) mod edge;
mod fast_cell;
pub(crate) mod freezer;
pub(crate) mod frozen;
pub(crate) mod maybe_uninit_slice_util;
pub(crate) mod module_heaps;
pub(crate) mod name;
pub(crate) mod owned_frozen;
pub(crate) mod owned_frozen_ext;
pub(crate) mod profile;
pub(crate) mod repr;
pub(crate) mod representation;
pub(crate) mod sealed;
pub(crate) mod send;
pub(crate) mod unfrozen;
