/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt::Debug;
use std::fmt::Display;
use std::hash::Hash;

use allocative::Allocative;
use async_trait::async_trait;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
// Re-exported so existing `dice::*ValueSerialize` paths (and `crate::api::key::*`)
// keep resolving after the definitions moved to the `pagable` crate.
pub use pagable::NoValueSerialize;
pub use pagable::OkPagableValueSerialize;
use pagable::Pagable;
use pagable::PagableTagged;
pub use pagable::PagableValueSerialize;
pub use pagable::TodoValueSerialize;
pub use pagable::ValueSerialize;

use crate::Demand;
use crate::api::computations::DiceComputations;
use crate::api::storage_type::StorageType;
use crate::introspection::graph::short_type_name;

/// How DICE determines whether a recomputed value equals the cached value.
pub enum EqualityBehavior<T> {
    /// Compare both values with the provided function.
    Compare(fn(&T, &T) -> bool),
    /// Every recomputation is treated as changed, so the cached value need not be materialized.
    AlwaysUnequal,
}

impl<T> EqualityBehavior<T> {
    pub(crate) fn is_always_unequal(&self) -> bool {
        matches!(self, Self::AlwaysUnequal)
    }
}

/// The computation Key that maps to a value. The key will be used as an index
/// for caching the computed values.
///
/// ## Cloning
/// The Key will need to be cloned. This occurs whenever the key is recomputed
/// and needs to be stored. During normal lookups, only a reference is needed.
///
/// Values are only ever handed out by reference, so they need not be cloneable at all. Callers
/// that want ownership of a value are responsible for cloning it themselves.
#[async_trait]
pub trait Key:
    Allocative + Debug + Display + Clone + Eq + Hash + Send + Sync + Pagable + PagableTagged + 'static
{
    type Value: Allocative + Send + Sync + 'static;

    /// Provides a short informative name for this key type.
    fn key_type_name() -> &'static str {
        short_type_name(std::any::type_name::<Self>())
    }

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellations: &CancellationContext,
    ) -> Self::Value;

    /// Defines how DICE determines whether a recomputed value equals the cached value.
    ///
    /// Returning equality for different values would produce inconsistent graph state. Keys whose
    /// values can never be reused should return [`EqualityBehavior::AlwaysUnequal`], which lets
    /// DICE avoid materializing a paged-out cached value after recomputation.
    fn equality_behavior() -> EqualityBehavior<Self::Value>;

    /// If the computed value is `false`, DICE will consider that result to be a transient value
    /// that won't be re-used on subsequent computations. It will, however, reuse that value for
    /// all on-going computations at the current version.
    ///
    /// The default here is true, but computations should override this if its expected that they
    /// may occasionally produce transient values.
    fn validity(_x: &Self::Value) -> bool {
        true
    }

    fn storage_type() -> StorageType {
        StorageType::Normal
    }

    /// Machinery to provide. Default implementation is no-op.
    ///
    /// Provided value can be obtained with [`DynKey`](crate::DynKey).
    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        let _ignore = demand;
    }

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::Normal
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value>;
}

/// What a key counts as when it is a source of invalidation, i.e. when it is injected or dirtied;
/// see [`crate::InvalidationSources`]. Every key takes part in propagating sources of both
/// priorities.
#[derive(Allocative, Debug, Clone, Copy, Dupe, PartialEq, Eq)]
pub enum InvalidationSourcePriority {
    /// The key will be ignored for invalidation source tracking.
    Ignored,
    /// The key can be an invalidation source only for the "normal" invalidation source path.
    Normal,
    /// The key can be an invalidation source for both the "normal" and "high" invalidation source paths.
    High,
}
