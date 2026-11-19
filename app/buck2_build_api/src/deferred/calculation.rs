/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Dice calculations relating to deferreds

use std::pin::Pin;
use std::sync::Arc;

use allocative::Allocative;
use buck2_artifact::actions::key::ActionKey;
use buck2_artifact::artifact::artifact_type::Artifact;
use buck2_core::deferred::base_deferred_key::BaseDeferredKey;
use buck2_core::deferred::base_deferred_key::BaseDeferredKeyDyn;
use buck2_core::deferred::key::DeferredHolderKey;
use buck2_node::rule_type::RuleType;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use dupe::Dupe;
use futures::Future;
use pagable::Pagable;
use starlark::values::OwnedFrozen;
use starlark::values::ValueTyped;

use crate::actions::RegisteredAction;
use crate::analysis::AnalysisResult;
use crate::analysis::calculation::RuleAnalysisCalculation;
use crate::analysis::registry::RecordedAnalysisValues;
use crate::artifact_groups::deferred::TransitiveSetKey;
use crate::artifact_groups::promise::PromiseArtifact;
use crate::bxl::calculation::BXL_CALCULATION_IMPL;
use crate::bxl::result::BxlResult;
use crate::dynamic::calculation::DynamicLambdaResult;
use crate::dynamic::calculation::dynamic_lambda_result;
use crate::interpreter::rule_defs::transitive_set::TransitiveSet;

pub static EVAL_ANON_TARGET: LateBinding<
    for<'c, 'd> fn(
        &'c mut DiceComputations<'d>,
        Arc<dyn BaseDeferredKeyDyn>,
    ) -> Pin<
        Box<dyn Future<Output = buck2_error::Result<&'d AnalysisResult>> + Send + 'c>,
    >,
> = LateBinding::new("EVAL_ANON_TARGET");

pub static GET_PROMISED_ARTIFACT: LateBinding<
    for<'c, 'd> fn(
        &'c PromiseArtifact,
        &'c mut DiceComputations<'d>,
    )
        -> Pin<Box<dyn Future<Output = buck2_error::Result<&'d Artifact>> + Send + 'c>>,
> = LateBinding::new("GET_PROMISED_ARTIFACT");

async fn lookup_deferred_inner<'d>(
    key: &BaseDeferredKey,
    dice: &mut DiceComputations<'d>,
) -> buck2_error::Result<DeferredHolder<'d>> {
    match key {
        BaseDeferredKey::TargetLabel(target) => {
            let analysis = dice
                .get_analysis_result(target)
                .await
                .require_compatible()?;

            Ok(DeferredHolder::Analysis(analysis))
        }
        BaseDeferredKey::BxlLabel(bxl) => {
            let bxl_result = BXL_CALCULATION_IMPL
                .get()?
                .eval_bxl(dice, bxl.dupe())
                .await?;

            Ok(DeferredHolder::Bxl(bxl_result))
        }
        BaseDeferredKey::AnonTarget(target) => Ok(DeferredHolder::Analysis(
            (EVAL_ANON_TARGET.get()?)(dice, target.dupe()).await?,
        )),
    }
}

pub async fn lookup_deferred_holder<'d>(
    dice: &mut DiceComputations<'d>,
    key: &DeferredHolderKey,
) -> buck2_error::Result<DeferredHolder<'d>> {
    Ok(match key {
        DeferredHolderKey::Base(key) => lookup_deferred_inner(key, dice).await?,
        DeferredHolderKey::DynamicLambda(lambda) => {
            DeferredHolder::DynamicLambda(dynamic_lambda_result(dice, lambda).await?)
        }
    })
}

/// Borrowing view of an Analysis, Bxl, or DynamicLambda result, held for the
/// duration of a dice transaction. Handed out on the hot lookup paths so that
/// resolving an action or transitive set doesn't need to clone the result out of
/// the graph. See [`OwnedDeferredHolder`] for the variant retained by the
/// detailed metrics tracker.
///
/// Technically, we can treat analysis as a 'Deferred' and get rid of this enum
#[derive(Copy, Clone, Dupe)]
pub enum DeferredHolder<'d> {
    Analysis(&'d AnalysisResult),
    Bxl(&'d Arc<BxlResult>),
    DynamicLambda(&'d Arc<DynamicLambdaResult>),
}

impl<'d> DeferredHolder<'d> {
    /// Read metadata from the holder already required to resolve an action.
    /// This must not demand the owner's configured node or deferred analysis fields.
    pub fn rule_type(self) -> Option<&'d RuleType> {
        match self {
            Self::Analysis(result) => Some(result.rule_type()),
            Self::Bxl(_) => None,
            Self::DynamicLambda(result) => result.rule_type.as_ref(),
        }
    }

    pub(crate) fn lookup_transitive_set(
        self,
        key: &TransitiveSetKey,
    ) -> buck2_error::Result<OwnedFrozen<ValueTyped<'static, TransitiveSet<'static>>>> {
        self.analysis_values()?.lookup_transitive_set(key)
    }

    pub(crate) fn lookup_action(self, key: &ActionKey) -> buck2_error::Result<ActionLookup> {
        self.analysis_values()?.lookup_action(key)
    }

    pub fn analysis_values(self) -> buck2_error::Result<&'d RecordedAnalysisValues> {
        match self {
            DeferredHolder::Analysis(result) => result.analysis_values(),
            DeferredHolder::Bxl(result) => Ok(result.analysis_values()),
            DeferredHolder::DynamicLambda(result) => Ok(result.analysis_values()),
        }
    }

    /// Clone the (cheaply dupe-able) inner handles into an owned holder.
    pub fn to_owned_holder(self) -> OwnedDeferredHolder {
        match self {
            DeferredHolder::Analysis(result) => OwnedDeferredHolder::Analysis(result.dupe()),
            DeferredHolder::Bxl(result) => OwnedDeferredHolder::Bxl(result.dupe()),
            DeferredHolder::DynamicLambda(result) => {
                OwnedDeferredHolder::DynamicLambda(result.dupe())
            }
        }
    }
}

/// Owned counterpart of [`DeferredHolder`], retained by the detailed metrics
/// tracker across the lifetime of the daemon (independent of the dice graph).
#[derive(Clone, Dupe)]
pub enum OwnedDeferredHolder {
    Analysis(AnalysisResult),
    Bxl(Arc<BxlResult>),
    DynamicLambda(Arc<DynamicLambdaResult>),
}

impl OwnedDeferredHolder {
    pub fn as_ref(&self) -> DeferredHolder<'_> {
        match self {
            OwnedDeferredHolder::Analysis(result) => DeferredHolder::Analysis(result),
            OwnedDeferredHolder::Bxl(result) => DeferredHolder::Bxl(result),
            OwnedDeferredHolder::DynamicLambda(result) => DeferredHolder::DynamicLambda(result),
        }
    }

    pub fn analysis_values(&self) -> buck2_error::Result<&RecordedAnalysisValues> {
        self.as_ref().analysis_values()
    }
}

#[derive(Debug, Allocative, Clone, Dupe, Pagable)]
pub enum ActionLookup {
    Action(Arc<RegisteredAction>),
    Deferred(ActionKey),
}
