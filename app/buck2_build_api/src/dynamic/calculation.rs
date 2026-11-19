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
use buck2_core::deferred::dynamic::DynamicLambdaResultsKey;
use buck2_node::rule_type::RuleType;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::FutureExt;
use futures::future::BoxFuture;
use pagable::Pagable;

use crate::analysis::registry::RecordedAnalysisValues;

#[derive(Allocative, Pagable)]
pub struct DynamicLambdaResult {
    pub analysis_values: RecordedAnalysisValues,
    /// Inherited from the parent holder; BXL-owned actions have no rule type.
    pub rule_type: Option<RuleType>,
}

impl DynamicLambdaResult {
    pub(crate) fn analysis_values(&self) -> &crate::analysis::registry::RecordedAnalysisValues {
        &self.analysis_values
    }
}

pub trait DynamicLambdaCalculation: Sync + 'static {
    fn dynamic_lambda_result<'a, 'd>(
        &self,
        dice: &'a mut DiceComputations<'d>,
        key: &'a DynamicLambdaResultsKey,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<DynamicLambdaResult>>>
    where
        'd: 'a;
}

pub static DYNAMIC_LAMBDA_CALCULATION_IMPL: LateBinding<&'static dyn DynamicLambdaCalculation> =
    LateBinding::new("DYNAMIC_LAMBDA_CALCULATION_IMPL");

pub fn dynamic_lambda_result<'a, 'd>(
    dice: &'a mut DiceComputations<'d>,
    key: &'a DynamicLambdaResultsKey,
) -> BoxFuture<'a, buck2_error::Result<&'d Arc<DynamicLambdaResult>>>
where
    'd: 'a,
{
    match DYNAMIC_LAMBDA_CALCULATION_IMPL.get() {
        Ok(v) => v.dynamic_lambda_result(dice, key),
        Err(e) => futures::future::ready(Err(e)).boxed(),
    }
}
