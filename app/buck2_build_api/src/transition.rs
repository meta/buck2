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
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::transition::applied::TransitionApplied;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_hash::BuckMutInsertionMap;
use buck2_node::attrs::configured_attr::ConfiguredAttr;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::future::BoxFuture;

#[async_trait]
pub trait TransitionCalculation: Send + Sync + 'static {
    /// Apply transition function to configuration and cache the result.
    async fn apply_transition(
        &self,
        ctx: &mut DiceComputations<'_>,
        attrs: &BuckMutInsertionMap<&str, Arc<ConfiguredAttr>>,
        conf: &ConfigurationData,
        transition_id: &TransitionId,
    ) -> buck2_error::Result<Arc<TransitionApplied>>;
}

pub static TRANSITION_CALCULATION: LateBinding<&'static dyn TransitionCalculation> =
    LateBinding::new("TRANSITION_CALCULATION");

pub static TRANSITION_ATTRS_PROVIDER: LateBinding<&'static dyn TransitionAttrProvider> =
    LateBinding::new("TRANSITION_ATTRS_PROVIDER");

//TODO transition attributes can be added to Rule. Basic idea is this:
// * in RuleCallable fetch TransitionId from transition value using TransitionValue trait
// * add a function like attrs to TransitionValue,
// * call it from RuleCallable, and store in Rule.
// * in TargetNode we have access to Rule.
pub trait TransitionAttrProvider: Send + Sync + 'static {
    /// Fetch attribute names accessed by transition function.
    fn transition_attrs<'a, 'd>(
        &self,
        ctx: &'a mut DiceComputations<'d>,
        transition_id: &'a TransitionId,
    ) -> BoxFuture<'a, buck2_error::Result<Option<&'d [String]>>>
    where
        'd: 'a;
}
