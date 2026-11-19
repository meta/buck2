/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! "Projection" keys: synchronously computed keys from "opaque" values.

use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;

use allocative::Allocative;
use dupe::Dupe;
use pagable::PagableDeserializeOwned;
use pagable::PagableSerialize;
use pagable::PagableTagged;

use crate::ValueSerialize;
use crate::api::data::DiceData;
use crate::api::key::EqualityBehavior;
use crate::api::key::Key;
use crate::api::storage_type::StorageType;
use crate::api::user_data::UserComputationData;
use crate::introspection::graph::short_type_name;

/// Synchronously computed key from an "opaque" value.
pub trait ProjectionKey:
    Allocative
    + Clone
    + PartialEq
    + Eq
    + Hash
    + Display
    + Debug
    + Send
    + Sync
    + PagableSerialize
    + PagableDeserializeOwned
    + PagableTagged
    + 'static
{
    /// Key of the value that this projection key is computed from.
    type DeriveFromKey: Key;
    /// This projection key is mapped to this value.
    type Value: Allocative + Dupe + Send + Sync;

    /// Compute the projection key value.
    // Implementation note:
    // This function is called from two places:
    // * `OpaqueValue::projection`, where the projection key is computed synchronously.
    // * `ProjectionKeyAsKey`, where the projection key is computed asynchronously
    //   together with opaque key this key is derived from.
    //   This operation cannot be triggered manually by user,
    //   but it happens implicitly during dependency recompute.
    fn compute(
        &self,
        derive_from: &<<Self as ProjectionKey>::DeriveFromKey as Key>::Value,
        ctx: &DiceProjectionComputations,
    ) -> Self::Value;

    /// Defines how DICE determines whether a recomputed value equals the cached value.
    ///
    /// Returning equality for different values would produce inconsistent graph state. Projections
    /// whose values can never be reused should return [`EqualityBehavior::AlwaysUnequal`].
    fn equality_behavior() -> EqualityBehavior<Self::Value>;

    fn validity(x: &Self::Value) -> bool {
        let _ = x;
        true
    }

    fn storage_type() -> StorageType {
        StorageType::Normal
    }

    /// Provides a short informative name for this projection type.
    fn key_type_name() -> &'static str {
        short_type_name(std::any::type_name::<Self>())
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value>;
}

/// Context for projection key computation.
///
/// Only provide access to globals.
pub struct DiceProjectionComputations<'a> {
    pub(crate) data: &'a DiceData,
    pub(crate) user_data: &'a UserComputationData,
}

impl DiceProjectionComputations<'_> {
    /// Untracked data that is static for the lifetime of this branch tree.
    pub fn global_data(&self) -> &DiceData {
        self.data
    }

    /// Data that is static for the lifetime of the current request context. This lifetime is
    /// the lifetime of the top-level `DiceComputation` used for all requests.
    /// The data is also specific to each request context, so multiple concurrent requests can
    /// each have their own individual data.
    pub fn per_transaction_data(&self) -> &UserComputationData {
        self.user_data
    }
}
