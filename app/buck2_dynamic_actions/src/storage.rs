/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_artifact::artifact::build_artifact::BuildArtifact;
use buck2_build_api::analysis::registry::AnalysisValueStorage;
use buck2_build_api::analysis::registry::FrozenAnalysisValueStorage;
use buck2_build_api::dynamic::storage::DYNAMIC_LAMBDA_PARAMS_STORAGES;
use buck2_build_api::dynamic::storage::DynamicLambdaParamStorages;
use buck2_build_api::dynamic::storage::DynamicLambdaParamsStorage;
use buck2_build_api::dynamic::storage::FrozenDynamicLambdaParamsStorage;
use buck2_build_api::dynamic::storage::FrozenDynamicLambdaParamsStorageBox;
use buck2_core::deferred::dynamic::DynamicLambdaIndex;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_error::BuckErrorOptionContext;
use buck2_error::internal_error;
use buck2_hash::BuckMutInsertionMap;
use dupe::Dupe;
use pagable::PagableDeserialize;
use pagable::PagableSerialize;
use starlark::any::AnyLifetime;
use starlark::any::ProvidesStaticType;
use starlark::pagable::StarlarkDeserialize;
use starlark::pagable::StarlarkDeserializeContext;
use starlark::pagable::StarlarkSerialize;
use starlark::pagable::StarlarkSerializeContext;
use starlark::values::DynStarlark;
use starlark::values::Freeze;
use starlark::values::FreezeResult;
use starlark::values::Freezer;
use starlark::values::OwnedFrozenRef;
use starlark::values::Trace;
use starlark::values::Tracer;

use crate::params::DynamicLambdaParams;
use crate::params::FrozenDynamicLambdaParams;

#[derive(Debug, Allocative, ProvidesStaticType)]
pub struct DynamicLambdaParamsStorageImpl<'v> {
    self_key: DeferredHolderKey,
    lambda_params: BuckMutInsertionMap<DynamicLambdaResultsKey, DynamicLambdaParams<'v>>,
}

#[derive(Debug, Allocative, ProvidesStaticType, starlark::StarlarkPagable)]
pub struct FrozenDynamicLambdaParamsStorageImpl<'fv> {
    // Mixed: `DynamicLambdaResultsKey` is pagable-only (`buck2_core` cannot
    // depend on `starlark`), values are starlark-aware — so the generic
    // `BuckMutInsertionMap<K, V>: StarlarkSerialize` blanket doesn't apply. Bridge here.
    #[starlark_pagable(
        serialize_with = "serialize_lambda_params",
        deserialize_with = "deserialize_lambda_params"
    )]
    lambda_params: BuckMutInsertionMap<DynamicLambdaResultsKey, FrozenDynamicLambdaParams<'fv>>,
}

fn serialize_lambda_params(
    field: &BuckMutInsertionMap<DynamicLambdaResultsKey, FrozenDynamicLambdaParams<'_>>,
    ctx: &mut dyn StarlarkSerializeContext,
) -> starlark::Result<()> {
    PagableSerialize::pagable_serialize(&field.len(), ctx.pagable())?;
    for (k, v) in field.iter() {
        PagableSerialize::pagable_serialize(k, ctx.pagable())?;
        StarlarkSerialize::starlark_serialize(v, ctx)?;
    }
    Ok(())
}

fn deserialize_lambda_params<'fv>(
    ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
) -> starlark::Result<BuckMutInsertionMap<DynamicLambdaResultsKey, FrozenDynamicLambdaParams<'fv>>>
{
    let len = usize::pagable_deserialize(ctx.pagable())?;
    let mut map = BuckMutInsertionMap::with_capacity(len);
    for _ in 0..len {
        let k =
            <DynamicLambdaResultsKey as PagableDeserialize>::pagable_deserialize(ctx.pagable())?;
        let v = FrozenDynamicLambdaParams::starlark_deserialize(ctx)?;
        map.insert(k, v);
    }
    Ok(map)
}

impl<'v> DynamicLambdaParamsStorageImpl<'v> {
    pub fn get<'a>(
        storage: &'a mut AnalysisValueStorage<'v>,
    ) -> buck2_error::Result<&'a mut DynamicLambdaParamsStorageImpl<'v>> {
        storage
            .lambda_params
            .as_any_mut()
            .downcast_mut()
            .internal_error("Wrong type for lambda params storage")
    }

    pub fn next_dynamic_actions_key(&self) -> buck2_error::Result<DynamicLambdaResultsKey> {
        let index = DynamicLambdaIndex::new(self.lambda_params.len().try_into()?);
        Ok(DynamicLambdaResultsKey::new(self.self_key.dupe(), index))
    }

    pub fn set_dynamic_actions(
        &mut self,
        key: DynamicLambdaResultsKey,
        lambda_params: DynamicLambdaParams<'v>,
    ) -> buck2_error::Result<()> {
        if &self.self_key != key.holder_key() {
            return Err(internal_error!(
                "Wrong lambda owner: expecting `{}`, got `{}`",
                self.self_key,
                key
            ));
        }
        self.lambda_params.insert(key, lambda_params);
        Ok(())
    }
}

impl FrozenDynamicLambdaParamsStorageImpl<'_> {
    pub fn lookup_lambda<'f>(
        storage: OwnedFrozenRef<'f, &'static FrozenAnalysisValueStorage<'static>>,
        key: &DynamicLambdaResultsKey,
    ) -> buck2_error::Result<OwnedFrozenRef<'f, &'static FrozenDynamicLambdaParams<'static>>> {
        if key.holder_key() != &storage.value().self_key {
            return Err(internal_error!(
                "Wrong owner for lambda: expecting `{}`, got `{}`",
                storage.value().self_key,
                key
            ));
        }
        storage.try_map::<&'static FrozenDynamicLambdaParams<'static>, buck2_error::Error, _>(|s| {
            s.lambda_params
                .as_any()
                .downcast_ref::<FrozenDynamicLambdaParamsStorageImpl>()
                .internal_error("Wrong type for lambda params storage")?
                .lambda_params
                .get(key)
                .with_internal_error(|| format!("missing lambda `{key}`"))
        })
    }
}

unsafe impl<'v> Trace<'v> for DynamicLambdaParamsStorageImpl<'v> {
    fn trace(&mut self, tracer: &Tracer<'v>) {
        let DynamicLambdaParamsStorageImpl {
            self_key,
            lambda_params,
        } = self;
        tracer.trace_static(self_key);
        for (k, v) in lambda_params.iter_mut() {
            tracer.trace_static(k);
            v.trace(tracer);
        }
    }
}

impl<'v> DynamicLambdaParamsStorage<'v> for DynamicLambdaParamsStorageImpl<'v> {
    fn as_any_mut(&mut self) -> &mut dyn AnyLifetime<'v> {
        self
    }

    fn freeze<'fv>(
        self: Box<DynStarlark<'v, Self>>,
        freezer: &Freezer<'v, 'fv>,
    ) -> FreezeResult<FrozenDynamicLambdaParamsStorageBox<'fv>> {
        let DynamicLambdaParamsStorageImpl {
            lambda_params,
            self_key: _,
        } = self.into_inner();
        // N.B. collect::<Result<_>> sets the lower bound to zero,
        // which can cause over-allocations in frozen containers.
        let mut frozen_lambda_params = BuckMutInsertionMap::with_capacity(lambda_params.len());
        for (k, v) in lambda_params.into_iter_hashed() {
            frozen_lambda_params.insert_hashed(k, v.freeze(freezer)?);
        }
        Ok(Box::new(DynStarlark::new(
            FrozenDynamicLambdaParamsStorageImpl {
                lambda_params: frozen_lambda_params,
            },
        )))
    }
}

impl<'fv> FrozenDynamicLambdaParamsStorage<'fv> for FrozenDynamicLambdaParamsStorageImpl<'fv> {
    fn as_any(&self) -> &dyn AnyLifetime<'fv> {
        self
    }

    fn dynamic_lambda_outputs(&self) -> Vec<BuildArtifact> {
        self.lambda_params
            .values()
            .flat_map(|v| {
                v.outputs
                    .iter()
                    .map(|a| a.as_ref().as_build_artifact().dupe())
            })
            .collect()
    }
}

pub fn init_dynamic_lambda_params_storages() {
    struct Impl;

    impl DynamicLambdaParamStorages for Impl {
        fn new_dynamic_lambda_params_storage<'v>(
            &self,
            self_key: DeferredHolderKey,
        ) -> Box<DynStarlark<'v, dyn DynamicLambdaParamsStorage<'v>>> {
            Box::new(DynStarlark::new(DynamicLambdaParamsStorageImpl {
                self_key,
                lambda_params: BuckMutInsertionMap::new(),
            }))
        }

        fn new_frozen_dynamic_lambda_params_storage<'fv>(
            &self,
        ) -> FrozenDynamicLambdaParamsStorageBox<'fv> {
            Box::new(DynStarlark::new(FrozenDynamicLambdaParamsStorageImpl {
                lambda_params: BuckMutInsertionMap::new(),
            }))
        }

        fn deserialize_frozen_dynamic_lambda_params_storage<'fv>(
            &self,
            ctx: &mut dyn StarlarkDeserializeContext<'_, 'fv>,
        ) -> starlark::Result<FrozenDynamicLambdaParamsStorageBox<'fv>> {
            Ok(Box::new(DynStarlark::new(
                FrozenDynamicLambdaParamsStorageImpl::starlark_deserialize(ctx)?,
            )))
        }
    }

    DYNAMIC_LAMBDA_PARAMS_STORAGES.init(&Impl);
}
