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
use buck2_core::package::PackageLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorContext;
use buck2_util::late_binding::LateBinding;
use buck2_util::time_span::TimeSpan;
use dice::DiceComputations;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::nodes::eval_result::EvaluationResult;
use crate::nodes::unconfigured::TargetNode;
use crate::super_package::SuperPackage;

#[async_trait]
pub trait TargetGraphCalculationImpl: Send + Sync + 'static {
    /// Like `get_interpreter_results`, but does not cache the result on the DICE graph, and
    /// evaluates through paths that do not record a dependency edge for everything they read.
    ///
    /// Must not be called from inside a `Key::compute`. The computed value would depend on
    /// reads that DICE cannot see, so it would never be invalidated when they change.
    async fn get_interpreter_results_uncached(
        &self,
        ctx: &mut DiceComputations<'_>,
        package: PackageLabel,
        cancellation: &CancellationContext,
    ) -> (TimeSpan, buck2_error::Result<Arc<EvaluationResult>>);

    /// Returns the full interpreter evaluation result for a Package. This consists of the full set
    /// of `TargetNode`s of interpreting that build file.
    fn get_interpreter_results<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        package: PackageLabel,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<EvaluationResult>>>
    where
        'd: 'a;
}

pub static TARGET_GRAPH_CALCULATION_IMPL: LateBinding<&'static dyn TargetGraphCalculationImpl> =
    LateBinding::new("TARGET_GRAPH_CALCULATION_IMPL");

#[async_trait]
pub trait TargetGraphCalculation<'d> {
    /// Like `get_interpreter_results`, but does not cache the result on the DICE graph, and
    /// evaluates through paths that do not record a dependency edge for everything they read.
    ///
    /// Must not be called from inside a `Key::compute`. The computed value would depend on
    /// reads that DICE cannot see, so it would never be invalidated when they change.
    async fn get_interpreter_results_uncached(
        &mut self,
        package: PackageLabel,
        cancellation: &CancellationContext,
    ) -> (TimeSpan, buck2_error::Result<Arc<EvaluationResult>>);

    /// Returns the full interpreter evaluation result for a Package. This consists of the full set
    /// of `TargetNode`s of interpreting that build file.
    fn get_interpreter_results<'a>(
        &'a mut self,
        package: PackageLabel,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<EvaluationResult>>>
    where
        'd: 'a;

    /// For a TargetLabel, returns the TargetNode. This is really just part of the interpreter
    /// results for the label's package, and so this is just a utility for accessing that, it
    /// isn't separately cached.
    fn get_target_node<'a>(
        &'a mut self,
        target: &'a TargetLabel,
    ) -> BoxFuture<'a, buck2_error::Result<TargetNode>>;

    /// For a TargetLabel, returns the TargetNode and its SuperPackage from PACKAGE files.
    fn get_target_node_with_super_package<'a>(
        &'a mut self,
        target: &'a TargetLabel,
    ) -> BoxFuture<'a, buck2_error::Result<(TargetNode, SuperPackage)>>;
}

#[async_trait]
impl<'d> TargetGraphCalculation<'d> for DiceComputations<'d> {
    async fn get_interpreter_results_uncached(
        &mut self,
        package: PackageLabel,
        cancellation: &CancellationContext,
    ) -> (TimeSpan, buck2_error::Result<Arc<EvaluationResult>>) {
        match TARGET_GRAPH_CALCULATION_IMPL.get() {
            Ok(calc) => {
                calc.get_interpreter_results_uncached(self, package, cancellation)
                    .await
            }
            Err(e) => (TimeSpan::empty_now(), Err(e)),
        }
    }

    fn get_interpreter_results<'a>(
        &'a mut self,
        package: PackageLabel,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Arc<EvaluationResult>>>
    where
        'd: 'a,
    {
        TARGET_GRAPH_CALCULATION_IMPL
            .get()
            .unwrap()
            .get_interpreter_results(self, package)
    }

    fn get_target_node<'a>(
        &'a mut self,
        target: &'a TargetLabel,
    ) -> BoxFuture<'a, buck2_error::Result<TargetNode>> {
        self.get_target_node_with_super_package(target)
            .map(|r| r.map(|(node, _)| node))
            .boxed()
    }

    fn get_target_node_with_super_package<'a>(
        &'a mut self,
        target: &'a TargetLabel,
    ) -> BoxFuture<'a, buck2_error::Result<(TargetNode, SuperPackage)>> {
        TARGET_GRAPH_CALCULATION_IMPL
            .get()
            .unwrap()
            .get_interpreter_results(self, target.pkg())
            .map(move |res| {
                let res = res.with_buck_error_context(|| {
                    format!(
                        "Error loading targets in package `{}` for target `{}`",
                        target.pkg(),
                        target
                    )
                })?;
                buck2_error::Ok((
                    res.resolve_target(target.name())?.to_owned(),
                    res.super_package().dupe(),
                ))
            })
            .boxed()
    }
}
