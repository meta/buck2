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

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::build::detailed_aggregated_metrics::dice::HasDetailedAggregatedMetrics;
use buck2_build_api::deferred::calculation::DeferredHolder;
use buck2_build_api::deferred::calculation::lookup_deferred_holder;
use buck2_build_api::dynamic::calculation::DYNAMIC_LAMBDA_CALCULATION_IMPL;
use buck2_build_api::dynamic::calculation::DynamicLambdaCalculation;
use buck2_build_api::dynamic::calculation::DynamicLambdaResult;
use buck2_build_signals::node_key::BuildSignalsNodeKey;
use buck2_build_signals::node_key::BuildSignalsNodeKeyImpl;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_data::ToProtoMessage;
use buck2_error::BuckErrorContext;
use dice::CancellationContext;
use dice::Demand;
use dice::DiceComputations;
use dice::EqualityBehavior;
use dice::Key;
use dice::OkPagableValueSerialize;
use dice::ValueSerialize;
use dupe::Dupe;
use dupe::ResultDupedErrExt;
use futures::FutureExt;
use futures::future::BoxFuture;
use pagable::Pagable;
use pagable::pagable_typetag;

use crate::deferred::prepare_and_execute_lambda;
use crate::storage::FrozenDynamicLambdaParamsStorageImpl;

struct DynamicLambdaCalculationImpl;

impl DynamicLambdaCalculation for DynamicLambdaCalculationImpl {
    fn dynamic_lambda_result<'a, 'd>(
        &self,
        dice: &'a mut DiceComputations<'d>,
        key: &'a DynamicLambdaResultsKey,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<DynamicLambdaResult>>>
    where
        'd: 'a,
    {
        async move {
            dice.compute(&DynamicLambdaDiceKey(key.dupe()))
                .await?
                .as_ref()
                .duped_err()
        }
        .boxed()
    }
}

pub fn init_dynamic_lambda_calculation() {
    DYNAMIC_LAMBDA_CALCULATION_IMPL.init(&DynamicLambdaCalculationImpl)
}

#[derive(
    Debug,
    derive_more::Display,
    Dupe,
    Clone,
    Allocative,
    Hash,
    Eq,
    PartialEq,
    Pagable
)]
#[pagable_typetag(dice::DiceKeyDyn)]
pub struct DynamicLambdaDiceKey(DynamicLambdaResultsKey);

#[async_trait]
impl Key for DynamicLambdaDiceKey {
    type Value = buck2_error::Result<Arc<DynamicLambdaResult>>;

    async fn compute(
        &self,
        ctx: &mut DiceComputations,
        cancellation: &CancellationContext,
    ) -> Self::Value {
        let self_deferred_key = DeferredHolderKey::DynamicLambda(Arc::new(self.0.dupe()));
        ctx.analysis_started(&self_deferred_key)?;

        let deferred_holder = lookup_deferred_holder(ctx, self.0.holder_key()).await?;
        let rule_type = deferred_holder
            .rule_type()
            .map(|rule_type| rule_type.dupe());
        let lambda = FrozenDynamicLambdaParamsStorageImpl::lookup_lambda(
            deferred_holder.analysis_values()?.analysis_storage()?,
            &self.0,
        )?;

        let analysis_values = prepare_and_execute_lambda(ctx, cancellation, lambda, self.0.dupe())
            .await
            .with_buck_error_context(|| {
                format!("Error running dynamic analysis for `{}`", self.0.owner())
            })?;
        let res = Arc::new(DynamicLambdaResult {
            analysis_values,
            rule_type,
        });
        ctx.analysis_complete(&self_deferred_key, &DeferredHolder::DynamicLambda(&res))?;
        Ok(res)
    }

    fn equality_behavior() -> EqualityBehavior<Self::Value> {
        EqualityBehavior::AlwaysUnequal
    }

    fn validity(x: &Self::Value) -> bool {
        x.is_ok()
    }

    fn provide<'a>(&'a self, demand: &mut Demand<'a>) {
        demand.provide_value_with(|| BuildSignalsNodeKey::new(self.dupe()));
    }

    fn value_serialize() -> impl ValueSerialize<Value = Self::Value> {
        OkPagableValueSerialize::<Self::Value>::new()
    }
}

impl BuildSignalsNodeKeyImpl for DynamicLambdaDiceKey {
    fn critical_path_entry_proto(&self) -> Option<buck2_data::critical_path_entry2::Entry> {
        let entry = buck2_data::critical_path_entry2::Entry::DynamicAnalysis(
            buck2_data::critical_path_entry2::DynamicAnalysis {
                target: match self.0.holder_key() {
                    DeferredHolderKey::Base(BaseDeferredKey::TargetLabel(target)) => Some(
                        buck2_data::critical_path_entry2::dynamic_analysis::Target::StandardTarget(
                            target.as_proto(),
                        ),
                    ),
                    _ => None,
                },
                index: self.0.dynamic_actions_index().as_u32(),
            },
        );

        Some(entry)
    }
}
