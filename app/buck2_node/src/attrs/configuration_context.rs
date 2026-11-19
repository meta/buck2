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

use buck2_core::configuration::compatibility::IncompatiblePlatformReason;
use buck2_core::configuration::compatibility::IncompatiblePlatformReasonCause;
use buck2_core::configuration::data::ConfigurationData;
use buck2_core::configuration::pair::ConfigurationNoExec;
use buck2_core::configuration::pair::ConfigurationWithExec;
use buck2_core::configuration::transition::applied::TransitionApplied;
use buck2_core::configuration::transition::id::TransitionId;
use buck2_core::execution_types::execution::ExecutionPlatformResolution;
use buck2_core::execution_types::execution_group::ExecutionGroup;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::provider::label::ConfiguredProvidersLabel;
use buck2_core::provider::label::ProvidersLabel;
use buck2_core::target::configured_target_label::ConfiguredTargetLabel;
use buck2_core::target::label::label::TargetLabel;
use buck2_error::BuckErrorOptionContext;
use buck2_hash::BuckFixedSortedMap;
use dupe::Dupe;

use crate::configuration::resolved::MatchedConfigurationSettingKeys;
use crate::configuration::resolved::MatchedConfigurationSettingKeysWithCfg;

#[derive(Debug, buck2_error::Error)]
#[buck2(tag = Tier0)]
pub enum PlatformConfigurationError {
    #[error("Could not find configuration for platform target `{0}`")]
    UnknownPlatformTarget(TargetLabel),
}

/// The context for attribute configuration. Contains information about the
/// configuration.
pub trait AttrConfigurationContext {
    fn matched_cfg_keys(&self) -> &MatchedConfigurationSettingKeys;

    fn cfg(&self) -> ConfigurationNoExec;

    fn base_exec_cfg(&self) -> buck2_error::Result<ConfigurationNoExec>;

    /// The execution platform resolution of the target whose attributes are being configured,
    /// when there is one.
    fn execution_platform_resolution(&self) -> Option<&ExecutionPlatformResolution> {
        None
    }

    /// Must be equal to `(cfg, Some(exec_cfg))`.
    fn toolchain_cfg(&self) -> ConfigurationWithExec;

    fn platform_cfg(&self, label: &TargetLabel) -> buck2_error::Result<ConfigurationData>;

    /// Map of transition ids resolved to configurations
    /// using current node configuration as input.
    fn resolved_transitions(
        &self,
    ) -> buck2_error::Result<&BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>>;

    /// Constructs an `IncompatiblePlatformReason` for this target from the given cause.
    fn incompatible_platform_reason(
        &self,
        cause: IncompatiblePlatformReasonCause,
    ) -> Arc<IncompatiblePlatformReason>;

    fn configure_target(&self, label: &ProvidersLabel) -> ConfiguredProvidersLabel {
        label.configure_pair(self.cfg().cfg_pair().dupe())
    }

    fn configure_exec_target(
        &self,
        label: &ProvidersLabel,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        Ok(label.configure_pair(self.base_exec_cfg()?.cfg_pair().dupe()))
    }

    fn configure_toolchain_target(&self, label: &ProvidersLabel) -> ConfiguredProvidersLabel {
        // The toolchain dependency itself is always configured in the target configuration,
        // but its exec_deps are considered when picking an execution platform, and MUST
        // use the execution dependency of its parent.
        label.configure_pair(self.toolchain_cfg().cfg_pair().dupe())
    }

    /// Configure an exec dep that belongs to a named execution group.
    fn configure_exec_target_in_group(
        &self,
        label: &ProvidersLabel,
        _group: &NamedExecutionGroup,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        self.configure_exec_target(label)
    }

    /// Configure a toolchain dep that belongs to a named execution group.
    fn configure_toolchain_target_in_group(
        &self,
        label: &ProvidersLabel,
        _group: &NamedExecutionGroup,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        Ok(self.configure_toolchain_target(label))
    }

    /// Configure a transition target.
    fn configure_transition_target(
        &self,
        label: &ProvidersLabel,
        tr: &TransitionId,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        let cfg = self
            .resolved_transitions()?
            .get(tr)
            .internal_error("internal error: no resolved transition")?;
        Ok(label.configure(cfg.single()?.dupe()))
    }

    fn configure_split_transition_target(
        &self,
        label: &ProvidersLabel,
        tr: &TransitionId,
    ) -> buck2_error::Result<BuckFixedSortedMap<String, ConfiguredProvidersLabel>> {
        let cfg = self
            .resolved_transitions()?
            .get(tr)
            .internal_error("internal error: no resolved transition")?;
        let split = cfg.split()?;
        Ok(split
            .iter()
            .map(|(k, v)| (k.to_owned(), label.configure(v.dupe())))
            .collect())
    }
}

pub struct AttrConfigurationContextImpl<'b> {
    configured_target_label: ConfiguredTargetLabel,
    resolved_cfg: &'b MatchedConfigurationSettingKeysWithCfg,
    /// Must be equal to `(cfg, Some(exec_cfg))`.
    toolchain_cfg: ConfigurationWithExec,
    resolved_transitions: &'b BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>,
    platform_cfgs: &'b BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    /// The execution platform resolution, which contains per-exec_dep configurations
    /// when in the Resolved state.
    execution_platform_resolution: &'b ExecutionPlatformResolution,
}

impl<'b> AttrConfigurationContextImpl<'b> {
    pub fn new(
        configured_target_label: ConfiguredTargetLabel,
        resolved_cfg: &'b MatchedConfigurationSettingKeysWithCfg,
        execution_platform_resolution: &'b ExecutionPlatformResolution,
        resolved_transitions: &'b BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>,
        platform_cfgs: &'b BuckFixedSortedMap<TargetLabel, ConfigurationData>,
    ) -> AttrConfigurationContextImpl<'b> {
        let exec_cfg = execution_platform_resolution.base_cfg();
        AttrConfigurationContextImpl {
            configured_target_label,
            resolved_cfg,
            toolchain_cfg: resolved_cfg.cfg().make_toolchain(&exec_cfg),
            resolved_transitions,
            platform_cfgs,
            execution_platform_resolution,
        }
    }
}

impl AttrConfigurationContext for AttrConfigurationContextImpl<'_> {
    fn matched_cfg_keys(&self) -> &MatchedConfigurationSettingKeys {
        self.resolved_cfg.settings()
    }

    fn cfg(&self) -> ConfigurationNoExec {
        self.resolved_cfg.cfg().dupe()
    }

    fn incompatible_platform_reason(
        &self,
        cause: IncompatiblePlatformReasonCause,
    ) -> Arc<IncompatiblePlatformReason> {
        Arc::new(IncompatiblePlatformReason {
            target: self.configured_target_label.dupe(),
            cause,
        })
    }

    fn base_exec_cfg(&self) -> buck2_error::Result<ConfigurationNoExec> {
        Ok(self.execution_platform_resolution.base_cfg())
    }

    fn execution_platform_resolution(&self) -> Option<&ExecutionPlatformResolution> {
        Some(self.execution_platform_resolution)
    }

    fn configure_exec_target(
        &self,
        label: &ProvidersLabel,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        let cfg = self
            .execution_platform_resolution
            .cfg_for_exec_dep(label.target())?;
        Ok(label.configure(cfg))
    }

    fn configure_exec_target_in_group(
        &self,
        label: &ProvidersLabel,
        group: &NamedExecutionGroup,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        let cfg = self
            .execution_platform_resolution
            .cfg_for_exec_dep_in_group(&ExecutionGroup::Named(group.dupe()), label.target())?;
        Ok(label.configure(cfg))
    }

    fn configure_toolchain_target_in_group(
        &self,
        label: &ProvidersLabel,
        group: &NamedExecutionGroup,
    ) -> buck2_error::Result<ConfiguredProvidersLabel> {
        let exec_cfg = self
            .execution_platform_resolution
            .base_cfg_for_group(&ExecutionGroup::Named(group.dupe()))?;
        Ok(label.configure_pair(
            self.resolved_cfg
                .cfg()
                .make_toolchain(&exec_cfg)
                .cfg_pair()
                .dupe(),
        ))
    }

    fn toolchain_cfg(&self) -> ConfigurationWithExec {
        self.toolchain_cfg.dupe()
    }

    fn platform_cfg(&self, label: &TargetLabel) -> buck2_error::Result<ConfigurationData> {
        match self.platform_cfgs.get(label) {
            Some(configuration) => Ok(configuration.dupe()),
            None => Err(PlatformConfigurationError::UnknownPlatformTarget(label.dupe()).into()),
        }
    }

    fn resolved_transitions(
        &self,
    ) -> buck2_error::Result<&BuckFixedSortedMap<Arc<TransitionId>, Arc<TransitionApplied>>> {
        Ok(self.resolved_transitions)
    }
}
