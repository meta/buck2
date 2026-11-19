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
use pagable::Pagable;
use pagable::PagableDeserializeOwned;
use pagable::PagableSerialize;
use pagable::typetag::PagableTagged;

use crate::InvalidationSourcePriority;
use crate::api::computations::DiceComputations;
use crate::api::key::EqualityBehavior;
use crate::api::key::Key;
use crate::api::key::ValueSerialize;
use crate::api::storage_type::StorageType;

/// Specialized version of `Key` above. This type of Key is never computed. It
/// should always be injected onto the graph before being requested via
/// `DiceCtx::changed_to(Key, Value)`. Therefore, the `compute` function on this
/// Key is never called.
///
/// TODO when rust gets smarter about negative trait bounds and specializations,
/// we can make injected and normal key types disjoint, and not have this clone issue nor
/// require a `panic!` implementation in `compute` function, both of which are
/// horrible and breaks semantics of traits.
pub trait InjectedKey:
    PagableSerialize
    + PagableDeserializeOwned
    + Allocative
    + Clone
    + Debug
    + Display
    + Send
    + Sync
    + Eq
    + Hash
    + Pagable
    + PagableTagged
    + 'static
{
    type Value: Allocative + Send + Sync + 'static;

    /// Defines how DICE determines whether a newly injected value equals the cached value.
    ///
    /// Returning equality for different values would produce inconsistent graph state. Injected
    /// keys whose values can never be reused should return [`EqualityBehavior::AlwaysUnequal`].
    fn equality_behavior() -> EqualityBehavior<Self::Value>;

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        InvalidationSourcePriority::Normal
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value>;
}

#[async_trait]
impl<K> Key for K
where
    K: InjectedKey,
{
    type Value = K::Value;

    async fn compute(
        &self,
        _ctx: &mut DiceComputations,
        _cancellations: &CancellationContext,
    ) -> Self::Value {
        panic!(
            "Injected Keys must be injected onto the graph before being requested, hence \
            computes should never be called; however, `compute` on `{:?}` was called.",
            self
        )
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        K::equality_behavior()
    }

    fn storage_type() -> StorageType {
        StorageType::Injected
    }

    fn invalidation_source_priority() -> InvalidationSourcePriority {
        K::invalidation_source_priority()
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        <K as InjectedKey>::value_serialize()
    }
}
