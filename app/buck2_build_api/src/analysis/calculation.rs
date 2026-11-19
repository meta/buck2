/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Rule analysis related Dice calculations
use std::future::Future;
use std::pin::Pin;

use async_trait::async_trait;
use buck2_core::configuration::compatibility::MaybeCompatible;
use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_hash::BuckMutMap;
use buck2_node::nodes::configured::ConfiguredTargetNode;
use buck2_node::nodes::configured_ref::ConfiguredGraphNodeRef;
use buck2_query::query::syntax::simple::eval::set::TargetSet;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::analysis::AnalysisResult;
use crate::interpreter::rule_defs::provider::collection::FrozenProviderCollectionValue;
use crate::validation::transitive_validations::TransitiveValidations;

pub static EVAL_ANALYSIS_QUERY: LateBinding<
    for<'a> fn(
        &'a mut DiceComputations,
        &'a str,
        BuckMutMap<String, ConfiguredTargetNode>,
    ) -> Pin<
        Box<
            dyn Future<Output = buck2_error::Result<TargetSet<ConfiguredGraphNodeRef>>> + Send + 'a,
        >,
    >,
> = LateBinding::new("EVAL_ANALYSIS_QUERY");

pub trait RuleAnalysisCalculationImpl: Send + Sync + 'static {
    /// Returns the analysis result for a ConfiguredTargetLabel. This is the full set of Providers
    /// returned by the target's rule implementation function.
    fn get_analysis_result<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d AnalysisResult>>
    where
        'd: 'a;
}

pub static RULE_ANALYSIS_CALCULATION: LateBinding<&'static dyn RuleAnalysisCalculationImpl> =
    LateBinding::new("RULE_ANALYSIS_CALCULATION");

#[async_trait]
pub trait RuleAnalysisCalculation<'d> {
    /// Returns the analysis result for a ConfiguredTargetLabel. This is the full set of Providers
    /// returned by the target's rule implementation function.
    fn get_analysis_result<'a>(
        &'a mut self,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d AnalysisResult>>
    where
        'd: 'a;

    /// Return the analysis result for a configuration rule `TargetLabel`
    /// (e. g. `constraint_value`).
    async fn get_configuration_analysis_result(
        &mut self,
        target: &ProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValue>;

    /// Returns the provider collection for a ConfiguredProvidersLabel. This is the full set of Providers
    /// returned by the target's rule implementation function.
    async fn get_providers(
        &mut self,
        target: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<MaybeCompatible<FrozenProviderCollectionValue>>;

    async fn get_validations(
        &mut self,
        target: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<Option<TransitiveValidations>>>;
}

#[async_trait]
impl<'d> RuleAnalysisCalculation<'d> for DiceComputations<'d> {
    fn get_analysis_result<'a>(
        &'a mut self,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d AnalysisResult>>
    where
        'd: 'a,
    {
        match RULE_ANALYSIS_CALCULATION.get() {
            Ok(calc) => calc.get_analysis_result(self, target),
            Err(e) => futures::future::ready(ResultMaybeCompatible::Err(e)).boxed(),
        }
    }

    async fn get_configuration_analysis_result(
        &mut self,
        target: &ProvidersLabel,
    ) -> buck2_error::Result<FrozenProviderCollectionValue> {
        // Analysis for configuration nodes is always done with the unbound configuration.
        let target = target.configure_pair(ConfigurationNoExec::unbound().cfg_pair().dupe());
        Ok(self.get_providers(&target).await?.require_compatible()?)
    }

    async fn get_providers(
        &mut self,
        target: &ConfiguredProvidersLabel,
    ) -> buck2_error::Result<MaybeCompatible<FrozenProviderCollectionValue>> {
        let analysis = self.get_analysis_result(target.target()).await.ok()?;

        analysis.try_map(|analysis| analysis.lookup_inner(target))
    }

    async fn get_validations(
        &mut self,
        target: &ConfiguredTargetLabel,
    ) -> buck2_error::Result<MaybeCompatible<Option<TransitiveValidations>>> {
        let analysis = self.get_analysis_result(target).await.ok()?;
        Ok(analysis.map(|x| x.validations.dupe()))
    }
}
