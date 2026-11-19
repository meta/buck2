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

//! Support for serialization and deserialization of Starlark values (pagable).
//!
//! This module provides the infrastructure needed to serialize and deserialize
//! Starlark heaps and values.
//!
//! ## Challenge: Type Inventory Registration
//!
//! During deserialization, we need to know which deserialization function to use
//! for each Starlark value type. This is solved by using the `inventory` crate
//! to register a mapping from type identifiers to vtables at compile time.
//!
//! The type identifier is `std::any::type_name<T>()` for the Rust type that
//! defines the Starlark value.
//!
//! ## Brands
//!
//! Deserialization happens at a brand (see the `branding` module): the framework introduces a
//! fresh `'fv` for each value it materializes, [`StarlarkDeserializeContext`] hands out
//! `Value<'fv>`s, and [`StarlarkDeserialize<'fv>`] is implemented by a value type at its own
//! brand. The framework writes the finished value into the heap being paged in, whose owner
//! later hands it out at a brand of that owner's; nothing an implementation of the trait can
//! write moves a value out of the heap it belongs to.

pub(crate) mod error;

#[cfg(feature = "pagable")]
pub(crate) mod vtable_registry;
#[cfg(not(feature = "pagable"))]
pub(crate) mod vtable_registry_stub;

#[cfg(feature = "pagable")]
pub(crate) use vtable_registry::DeserTypeId;
#[cfg(feature = "pagable")]
pub(crate) use vtable_registry::lookup_vtable;
#[cfg(not(feature = "pagable"))]
pub(crate) use vtable_registry_stub::DeserTypeId;
#[cfg(not(feature = "pagable"))]
pub(crate) use vtable_registry_stub::lookup_vtable;

/// Runtime-registration marker trait for vtable lookup at deserialize time.
pub mod vtable_register;

pub use vtable_register::VtableRegistered;

pub(crate) mod static_value;
pub use static_value::StaticHeapEntry;
pub use static_value::StaticValueEntry;
pub use static_value::StaticValueRegistered;
#[allow(unused_imports)]
pub(crate) use static_value::get_static_value_id;

pub(crate) mod heap_ref_id;
pub(crate) mod serialized_frozen_value;
pub(crate) mod starlark_deserialize;
pub(crate) mod starlark_deserialize_context;
pub(crate) mod starlark_pagable;
pub(crate) mod starlark_serialize;
pub(crate) mod starlark_serialize_context;

mod starlark_pagable_impls;

// Re-export public types
pub use starlark_deserialize::StarlarkDeserialize;
pub use starlark_deserialize::StarlarkDeserializeAt;
pub use starlark_deserialize::StarlarkDeserializeContext;
pub use starlark_deserialize::starlark_deserialize_field;
pub use starlark_deserialize_context::DeferredFieldReadsEnabled;
pub use starlark_deserialize_context::PartialDeserStats;
#[doc(hidden)]
pub use starlark_deserialize_context::starlark_deserialization_state_retained_bytes;
pub use starlark_deserialize_context::starlark_partial_deser_stats;
pub use starlark_pagable::StarlarkPagable;
pub use starlark_pagable_impls::SmallMapKeyDeserialize;
pub use starlark_serialize::StarlarkSerialize;
pub use starlark_serialize::StarlarkSerializeContext;
#[doc(hidden)]
pub use starlark_serialize::StarlarkSerializeScope;
pub use starlark_serialize_context::StarlarkSerializerImpl;
#[doc(hidden)]
pub use starlark_serialize_context::starlark_serialization_state_retained_bytes;

#[doc(hidden)]
pub use crate::values::deferred::DeferredReadContext;
#[doc(hidden)]
pub use crate::values::layout::heap::sealed::heap_key_index::take_heap_rekeys;

#[cfg(all(test, feature = "pagable"))]
mod state_benchmark;
#[cfg(all(test, feature = "pagable"))]
mod tests;
