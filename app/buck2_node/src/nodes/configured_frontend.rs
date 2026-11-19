/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_core::configuration::compatibility::ResultMaybeCompatible;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::nodes::configured::ConfiguredTargetNode;

pub trait ConfiguredTargetNodeCalculationImpl: Send + Sync + 'static {
    /// Returns the ConfiguredTargetNode corresponding to a ConfiguredTargetLabel.
    fn get_configured_target_node<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        target: &'a ConfiguredTargetLabel,
        check_dependency_incompatibility: bool,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d ConfiguredTargetNode>>
    where
        'd: 'a;
}

pub static CONFIGURED_TARGET_NODE_CALCULATION: LateBinding<
    &'static dyn ConfiguredTargetNodeCalculationImpl,
> = LateBinding::new("CONFIGURED_TARGET_NODE_CALCULATION");

pub trait ConfiguredTargetNodeCalculation<'d> {
    /// Returns the ConfiguredTargetNode corresponding to a ConfiguredTargetLabel.
    fn get_configured_target_node<'a>(
        &'a mut self,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d ConfiguredTargetNode>>
    where
        'd: 'a;

    /// Same as `get_configured_target_node` except it doesn't error/soft-error on
    /// configured target that is transitively incompatible. This should only be used
    /// to obtain any configured target node used as deps of other configured nodes,
    /// ex. recursively from `get_configured_target_node` function. All other use cases
    /// should use `get_configured_target_node` instead.
    fn get_internal_configured_target_node<'a>(
        &'a mut self,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d ConfiguredTargetNode>>
    where
        'd: 'a;
}

impl<'d> ConfiguredTargetNodeCalculation<'d> for DiceComputations<'d> {
    fn get_configured_target_node<'a>(
        &'a mut self,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d ConfiguredTargetNode>>
    where
        'd: 'a,
    {
        match CONFIGURED_TARGET_NODE_CALCULATION.get() {
            Ok(calc) => calc.get_configured_target_node(self, target, true),
            Err(e) => futures::future::ready(ResultMaybeCompatible::Err(e)).boxed(),
        }
    }

    fn get_internal_configured_target_node<'a>(
        &'a mut self,
        target: &'a ConfiguredTargetLabel,
    ) -> BoxFuture<'a, ResultMaybeCompatible<&'d ConfiguredTargetNode>>
    where
        'd: 'a,
    {
        match CONFIGURED_TARGET_NODE_CALCULATION.get() {
            Ok(calc) => calc.get_configured_target_node(self, target, false),
            Err(e) => futures::future::ready(ResultMaybeCompatible::Err(e)).boxed(),
        }
    }
}
