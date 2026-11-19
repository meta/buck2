/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use async_trait::async_trait;
use buck2_build_api::bxl::calculation::BXL_CALCULATION_IMPL;
use buck2_build_api::bxl::calculation::BxlCalculationDyn;
use buck2_build_api::bxl::result::BxlResult;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyBxl;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::FutureExt;

use crate::bxl;
use crate::bxl::eval::eval;
use crate::bxl::key::BxlKey;

#[derive(Debug)]
struct BxlCalculationImpl;

impl BxlCalculationDyn for BxlCalculationImpl {
    fn eval_bxl<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        bxl: BaseDeferredKeyBxl,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<BxlResult>>>
    where
        'd: 'a,
    {
        async move {
            let key = BxlKey::from_base_deferred_key_dyn_impl_err(bxl)?;
            eval_bxl_ref(ctx, key).await
        }
        .boxed()
    }
}

pub(crate) fn init_bxl_calculation_impl() {
    BXL_CALCULATION_IMPL.init(&BxlCalculationImpl);
}

pub(crate) async fn eval_bxl(
    ctx: &mut DiceComputations<'_>,
    bxl: BxlKey,
) -> bxl::eval::Result<Arc<BxlResult>> {
    match ctx.compute(&internal::BxlComputeKey(bxl)).await {
        Ok(res) => res.dupe(),
        Err(e) => Err(buck2_error::Error::from(e).into()),
    }
}

/// Like [`eval_bxl`] but borrows the result out of the dice graph rather than
/// duping the `Arc` out of it; the bxl-eval error is surfaced as a plain error.
pub(crate) async fn eval_bxl_ref<'d>(
    ctx: &mut DiceComputations<'d>,
    bxl: BxlKey,
) -> buck2_error::Result<&'d Arc<BxlResult>> {
    ctx.compute(&internal::BxlComputeKey(bxl))
        .await?
        .as_ref()
        .map_err(|e| e.error.dupe())
}

#[async_trait]
impl Key for internal::BxlComputeKey {
    type Value = bxl::eval::Result<Arc<BxlResult>>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellation: &CancellationContext,
    ) -> Self::Value {
        let key = self.0.dupe();
        // TODO(cjhopman): send analysis started/finished events for bxl to support detailed aggregated metrics
        cancellation
            .with_structured_cancellation(|observer| {
                async move {
                    eval(ctx, key, observer)
                        .await
                        .map(|(result, _)| Arc::new(result))
                }
                .boxed()
            })
            .await
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::AlwaysUnequal
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

mod internal {
    use allocative::Allocative;
    use derive_more::Display;
    use dupe::Dupe;
    use pagable::Pagable;
    use pagable::pagable_typetag;

    use crate::bxl::key::BxlKey;

    #[derive(Clone, Dupe, Display, Debug, Eq, Hash, PartialEq, Allocative, Pagable)]
    #[pagable_typetag(dice::DiceKeyDyn)]
    pub(crate) struct BxlComputeKey(pub(crate) BxlKey);
}
