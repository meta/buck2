/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use async_trait::async_trait;
use buck2_core::global_cfg_options::GlobalCfgOptions;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use dupe::Dupe;

use crate::nodes::configured_frontend::ConfiguredTargetNodeCalculation;

#[async_trait]
pub trait ConfiguredTargetCalculationImpl: Send + Sync + 'static {
    async fn get_configured_target(
        &self,
        ctx: &mut DiceComputations<'_>,
        target: &TargetLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredTargetLabel>;
}

pub static CONFIGURED_TARGET_CALCULATION: LateBinding<
    &'static dyn ConfiguredTargetCalculationImpl,
> = LateBinding::new("CONFIGURED_TARGET_CALCULATION");

#[async_trait]
pub trait ConfiguredTargetCalculation {
    /// Returns the Configuration for an unconfigured TargetLabel or ProvidersLabel.
    ///
    /// This performs "target platform resolution" on the provided target and returns the configured
    /// result.
    ///
    /// Normally, a TargetLabel and ProvidersLabel would
    /// get its Configuration based on the context it's being requested in (i.e configuration is
    /// passed down from higher nodes). For top-level requested things, though, we will have an
    /// unconfigured (or "lightly"-configured) thing and the Configuration will be determined as
    /// a mix of the global Configuration, the target's `default_target_platform` and
    /// (potentially) self-transitions on that node.
    async fn get_configured_target(
        &mut self,
        target: &TargetLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredTargetLabel>;

    async fn get_configured_target_post_transition(
        &mut self,
        target: &TargetLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredTargetLabel>;

    async fn get_configured_provider_label(
        &mut self,
        target: &ProvidersLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredProvidersLabel>;

    async fn get_default_configured_target(
        &mut self,
        target: &TargetLabel,
    ) -> buck2_error::Result<ConfiguredTargetLabel>;
}

#[async_trait]
impl ConfiguredTargetCalculation for DiceComputations<'_> {
    async fn get_configured_target(
        &mut self,
        target: &TargetLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredTargetLabel> {
        CONFIGURED_TARGET_CALCULATION
            .get()?
            .get_configured_target(self, target, global_cfg_options)
            .await
    }

    async fn get_configured_target_post_transition(
        &mut self,
        target: &TargetLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredTargetLabel> {
        let configured = self
            .get_configured_target(target, global_cfg_options)
            .await?;
        let configured_target_node = self
            .get_internal_configured_target_node(&configured)
            .await
            .require_compatible()?;
        Ok(configured_target_node.unwrap_forward().label().dupe())
    }

    async fn get_configured_provider_label(
        &mut self,
        target: &ProvidersLabel,
        global_cfg_options: &GlobalCfgOptions,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        let configured_target_label = CONFIGURED_TARGET_CALCULATION
            .get()?
            .get_configured_target(self, target.target(), global_cfg_options)
            .await?;
        Ok(ConfiguredProvidersLabel::new(
            configured_target_label,
            target.name().clone(),
        ))
    }

    async fn get_default_configured_target(
        &mut self,
        target: &TargetLabel,
    ) -> buck2_error::Result<ConfiguredTargetLabel> {
        CONFIGURED_TARGET_CALCULATION
            .get()?
            .get_configured_target(self, target, &GlobalCfgOptions::default())
            .await
    }
}
