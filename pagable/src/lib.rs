/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Pagable serialization framework.
//!
//! The main traits for types to implement are:
//! - [`PagableSerialize`] - implemented by types that can be serialized
//! - [`PagableDeserialize`] - implemented by types that can be deserialized
//! - [`PagableDeserializeOwned`] - implemented by types that can be deserialized from any lifetime
//! - [`Pagable`] - a convenience trait combining both PagableSerialize and PagableDeserialize with `Send + Sync + Debug`
//!
//! Implementations can use the derive macros `#[derive(Pagable)]` (which will derive both serialize and deserialize), `#[derive(PagableSerialize)]`,
//! and `#[derive(PagableDeserialize)]` for automatic implementations.

pub use pagable_arc::PagableArc;
pub use pagable_arc::PinnedPagableArc;
pub use pagable_arc::PinnedPagableArcBorrow;
pub use pagable_derive::Pagable;
pub use pagable_derive::PagableDeserialize;
pub use pagable_derive::PagablePanic;
pub use pagable_derive::PagableSerialize;
pub use pagable_derive::PagableUnsupported;
pub use pagable_derive::pagable_tagged;
pub use pagable_derive::pagable_typetag;
pub use partial_pagable_arc::PartialPagableArc;
pub use partial_pagable_arc::PartialPagableWeak;
pub use typetag::GenericTypetagAccumulator;
pub use typetag::PagableRegisteredFor;
pub use typetag::PagableStableName;
pub use typetag::PagableTagged;
pub use typetag::PagableTypeTag;

pub mod arc_erase;
pub mod context;
mod deferred_value;
pub mod deser_recipe;
pub mod flavors;
mod hashers;
mod impls;
mod pagable_arc;
mod partial_pagable_arc;
#[cfg(feature = "tokio")]
pub mod prepare;
mod read_failures;
pub mod storage;
#[cfg(test)]
mod test;
pub mod testing;
pub mod traits;
pub mod typetag;
pub mod value_serialize;

pub use deferred_value::DeferredValue;
pub use deser_recipe::PagableDeserializerRecipe;
pub use deser_recipe::PagableDeserializerRecipeImpl;
pub use impls::StaticStr;
pub use impls::static_value;
pub use impls::static_value::StaticBytes;
pub use impls::static_value::StaticValue;
pub use read_failures::DeferredReadError;
pub use read_failures::DeferredReadFailures;
pub use read_failures::ReadRefused;
pub use read_failures::ReadTimedOut;
pub use read_failures::is_transient_read_error;
pub use storage::data::DataKey;
pub use traits::Pagable;
pub use traits::PagableBoxDeserialize;
pub use traits::PagableCursor;
pub use traits::PagableDeserialize;
pub use traits::PagableDeserializeOwned;
pub use traits::PagableDeserializer;
pub use traits::PagableEagerDeserialize;
pub use traits::PagableEagerSerialize;
pub use traits::PagableSerialize;
pub use traits::PagableSerializer;
pub use traits::StorageContext;
pub use traits::StorageState;
pub use value_serialize::NoValueSerialize;
pub use value_serialize::OkPagableValueSerialize;
pub use value_serialize::PagableValueSerialize;
pub use value_serialize::TodoValueSerialize;
pub use value_serialize::ValueSerialize;

pub type Result<O> = anyhow::Result<O>;
pub type Error = anyhow::Error;

pub mod __internal {
    pub use std::cell::Cell;

    pub use anyhow;
    pub use inventory;
    pub use once_cell;
    pub use serde;
    pub use static_assertions;
}

// ast-grep-ignore: rust/buck2-no-use-anyhow
pub use anyhow::anyhow;
