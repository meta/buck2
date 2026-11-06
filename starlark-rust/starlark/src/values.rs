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

//! Defines a runtime Starlark value ([`Value`]) and traits for defining custom values ([`StarlarkValue`]).
//!
//! This module contains code for working with Starlark values:
//!
//! * Most code dealing with Starlark will use [`Value`], as it represents the fundamental values used in
//!   Starlark.
//! * Values are garbage-collected, so a given [`Value`] lives on a [`Heap`].
//! * Rust values (e.g. [`String`], [`Vec`]) can be added to the [`Heap`] with [`AllocValue`],
//!   and deconstructed from a [`Value`] with [`UnpackValue`]
//!   (or specialised methods like [`unpack_str`](Value::unpack_str)).
//! * To define your own Rust data type that can live in a [`Value`] it must implement the [`StarlarkValue`]
//!   trait.
//! * All the nested modules represent the built-in Starlark values. These are all defined using [`StarlarkValue`],
//!   so may serve as interesting inspiration for writing your own values, in addition to occurring in Starlark programs.
//!
//! # Frozen and unfrozen values
//!
//! A Starlark module is evaluated on one thread, and once it has finished evaluating, everything
//! it defines is immutable: a list defined by one module cannot be mutated by the modules that
//! `load()` it. That is what lets a loaded module be shared by many importers, on many threads,
//! without being copied.
//!
//! The runtime mirrors the two phases. While a module runs, its values are allocated on the
//! module's [`Heap`], where they may be mutable and are garbage collected. Freezing the module
//! ([`Module::freeze_named`](crate::environment::Module::freeze_named)) copies every value
//! reachable from the module's variables onto the module's frozen heap through [`Freeze`];
//! the copies are immutable, and a value reachable from several places is copied once, see
//! [`Freezer`]. The sealed frozen heap ([`OwnedFrozen`]) is `Send + Sync` and is shared by
//! reference.
//!
//! A `Value<'v>` on a module's heap may be either frozen or unfrozen, which
//! [`is_frozen`](Value::is_frozen) tells apart; a value at the brand of a frozen heap is always
//! frozen. Inline integers and statics (`None`, the booleans, the empty string and the empty
//! containers) live in no heap and count as frozen.
//!
//! The lifetime `'v` of a `Value<'v>` identifies the heap the value lives in, or a heap that heap
//! keeps alive; it does not measure how long anything lives. The `branding` module in
//! `values/layout/heap/branding.rs` explains that discipline, and the `heap` module in
//! `values/layout/heap.rs` describes the heaps and how values move between them.

pub use starlark_derive::AllocFrozenValue;
pub use starlark_derive::AllocValue;
pub use starlark_derive::Freeze;
pub use starlark_derive::NoSerialize;
pub use starlark_derive::StarlarkPagable;
pub use starlark_derive::StarlarkPagablePanic;
pub use starlark_derive::StarlarkPagableUnsupported;
pub use starlark_derive::StarlarkPagableViaPagable;
pub use starlark_derive::StarlarkRepresentations;
pub use starlark_derive::Trace;
pub use starlark_derive::UnpackValue;
pub use starlark_derive::starlark_value;

pub use crate::any::AnyLifetime;
pub use crate::any::ProvidesStaticType;
pub use crate::pagable::StaticValueRegistered;
pub use crate::values::alloc_value::AllocFrozenValue;
pub use crate::values::alloc_value::AllocValue;
pub use crate::values::deferred::Deferred;
pub use crate::values::deferred::DeferredWord;
pub use crate::values::demand::Demand;
pub use crate::values::error::ValueError;
pub use crate::values::freeze::Freeze;
pub use crate::values::freeze::StaticFreezePlan;
pub use crate::values::freeze_dynamic::FreezeDynamic;
pub use crate::values::freeze_dynamic::FreezePlan;
pub use crate::values::freeze_dynamic::FreezeSlot;
pub use crate::values::freeze_dynamic::FreezeTarget;
pub use crate::values::freeze_error::FreezeError;
pub use crate::values::freeze_error::FreezeErrorContext;
pub use crate::values::freeze_error::FreezeResult;
pub use crate::values::iter::StarlarkIterator;
pub use crate::values::layout::avalues::static_::AllocStaticSimple;
pub use crate::values::layout::heap::edge::HeapEdge;
pub(crate) use crate::values::layout::heap::edge::SealEdge;
pub use crate::values::layout::heap::freezer::Freezer;
pub use crate::values::layout::heap::frozen::FrozenHeap;
pub use crate::values::layout::heap::frozen::OwnedFrozenHeap;
pub use crate::values::layout::heap::name::FrozenHeapName;
pub use crate::values::layout::heap::name::SingletonFrozenHeapName;
pub use crate::values::layout::heap::name::StringUserHeapName;
pub use crate::values::layout::heap::name::UserHeapName;
pub use crate::values::layout::heap::owned_frozen::OwnedFrozen;
pub use crate::values::layout::heap::owned_frozen::OwnedFrozenReconstructor;
pub use crate::values::layout::heap::owned_frozen::OwnedFrozenRef;
pub use crate::values::layout::heap::owned_frozen_ext::DeferredOwnedFrozen;
pub use crate::values::layout::heap::owned_frozen_ext::FnOncish;
pub use crate::values::layout::heap::owned_frozen_ext::FnOncish2;
pub use crate::values::layout::heap::send::DynStarlark;
pub use crate::values::layout::heap::send::HeapSendable;
pub use crate::values::layout::heap::send::HeapSyncable;
pub use crate::values::layout::heap::unfrozen::Heap;
pub use crate::values::layout::heap::unfrozen::Tracer;
pub use crate::values::layout::identity::ValueIdentity;
pub use crate::values::layout::static_string::StarlarkStrNRepr;
pub use crate::values::layout::static_string::constant_string;
pub use crate::values::layout::typed::FrozenValueTyped;
pub use crate::values::layout::typed::ValueTyped;
pub use crate::values::layout::typed::string::StringValue;
pub use crate::values::layout::value::Value;
pub use crate::values::thin_box_slice_value::packed_impl::ThinBoxSliceValue;
pub use crate::values::trace::Trace;
pub use crate::values::traits::ComplexValue;
pub use crate::values::traits::StarlarkType;
pub use crate::values::traits::StarlarkValue;
pub use crate::values::types::any;
pub use crate::values::types::any_complex;
pub use crate::values::types::array;
pub use crate::values::types::bool;
pub use crate::values::types::bytes;
pub use crate::values::types::dict;
pub use crate::values::types::enumeration;
pub use crate::values::types::float;
pub use crate::values::types::function;
pub use crate::values::types::int;
pub use crate::values::types::list;
pub use crate::values::types::list_or_tuple;
pub use crate::values::types::namespace;
pub use crate::values::types::none;
pub use crate::values::types::range;
pub use crate::values::types::record;
pub use crate::values::types::set;
pub use crate::values::types::starlark_value_as_type;
pub use crate::values::types::string;
pub use crate::values::types::structs;
pub use crate::values::types::tuple;
pub use crate::values::unpack::UnpackValue;
pub use crate::values::unpack::UnpackValueError;
pub use crate::values::unpack::UnpackValueErrorInfallible;
pub use crate::values::unpack_and_discard::UnpackAndDiscard;
pub use crate::values::value_of::ValueOf;
pub use crate::values::value_of_unchecked::ValueOfUnchecked;

mod alloc_value;
mod comparison;
pub(crate) mod deferred;
pub(crate) mod demand;
pub(crate) mod error;
mod freeze;
pub(crate) mod freeze_dynamic;
mod freeze_error;
mod index;
pub(crate) mod iter;
pub(crate) mod layout;
pub(crate) mod recursive_repr_or_json_guard;
mod stack_guard;
pub(crate) mod starlark_type_id;
pub(crate) mod thin_box_slice_value;
mod trace;
pub(crate) mod traits;
pub mod type_repr;
pub(crate) mod types;
pub mod typing;
mod unpack;
mod unpack_and_discard;
pub(crate) mod value_of;
pub(crate) mod value_of_unchecked;
