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

use allocative::Allocative;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_util::late_binding::LateBinding;
use starlark::any::AnyLifetime;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::pagable::StarlarkSerialize;
use starlark::values::DynStarlark;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::HeapSendable;
use starlark::values::HeapSyncable;
use starlark::values::Trace;

/// The frozen form of this storage, at the brand of the heap that keeps it alive.
pub type FrozenDynamicLambdaParamsStorageBox<'fv> =
    Box<DynStarlark<'fv, dyn FrozenDynamicLambdaParamsStorage<'fv>>>;

pub trait DynamicLambdaParamsStorage<'v>:
    HeapSendable<'v> + Trace<'v> + Debug + Allocative + 'v
{
    fn as_any_mut(&mut self) -> &mut dyn AnyLifetime<'v>;

    fn freeze<'fv>(
        self: Box<DynStarlark<'v, Self>>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<FrozenDynamicLambdaParamsStorageBox<'fv>>;
}

pub trait FrozenDynamicLambdaParamsStorage<'fv>:
    Debug + Allocative + StarlarkSerialize + HeapSendable<'fv> + HeapSyncable<'fv> + 'fv
{
    fn as_any(&self) -> &dyn AnyLifetime<'fv>;

    /// Collected rather than lazily iterated: the params borrow their heap's brand, which is
    /// `Send`/`Sync` only at `'static`, and callers hold the result across awaits.
    fn dynamic_lambda_outputs(&self) -> Vec<BuildArtifact>;
}

pub trait DynamicLambdaParamStorages: Send + Sync + 'static {
    fn new_dynamic_lambda_params_storage<'v>(
        &self,
        self_key: DeferredHolderKey,
    ) -> Box<DynStarlark<'v, dyn DynamicLambdaParamsStorage<'v>>>;

    fn new_frozen_dynamic_lambda_params_storage<'fv>(
        &self,
    ) -> FrozenDynamicLambdaParamsStorageBox<'fv>;

    /// Counterpart to [`StarlarkSerialize`], which the frozen storage implements directly.
    ///
    /// Deserialization cannot go through a pagable typetag registry: the registry hands back a
    /// single lifetime-erased `Box<dyn Trait>`, and there is no safe way to brand that. Going
    /// through the late binding keeps the brand a parameter of the call.
    fn deserialize_frozen_dynamic_lambda_params_storage<'fv>(
        &self,
        ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
    ) -> starlark::Result<FrozenDynamicLambdaParamsStorageBox<'fv>>;
}

pub static DYNAMIC_LAMBDA_PARAMS_STORAGES: LateBinding<&'static dyn DynamicLambdaParamStorages> =
    LateBinding::new("DYNAMIC_LAMBDA_PARAMS_STORAGES");
