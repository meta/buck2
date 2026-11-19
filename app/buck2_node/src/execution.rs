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
use buck2_core::execution_types::execution::ExecutionPlatformIncompatibleReason;
use buck2_core::execution_types::execution::ExecutionPlatformResolutionPartial;
use buck2_core::execution_types::execution_group::NamedExecutionGroup;
use buck2_core::execution_types::execution_platforms::ExecutionPlatforms;
use buck2_core::target::label::label::TargetLabel;
use buck2_core::target::target_configured_target_label::TargetConfiguredTargetLabel;
use buck2_util::late_binding::LateBinding;
use dice::DiceComputations;
use futures::FutureExt;
use futures::future::BoxFuture;

use crate::configuration::calculation::CellNameForConfigurationResolution;
use crate::configuration::resolved::ConfigurationSettingKey;

/// The outcome of checking each candidate execution platform, by platform name.
pub type ExecutionPlatformCandidateOutcomes =
    Vec<(String, Result<(), ExecutionPlatformIncompatibleReason>)>;

/// The candidate outcomes of every execution group of a target. Each group, the default one
/// included, has its own result, so a group whose diagnosis fails does not hide the others'.
pub struct ExecutionPlatformDiagnosis {
    pub default: buck2_error::Result<ExecutionPlatformCandidateOutcomes>,
    /// One entry per named execution group the rule declares, in declaration order.
    pub execution_groups: Vec<(
        NamedExecutionGroup,
        buck2_error::Result<ExecutionPlatformCandidateOutcomes>,
    )>,
}

#[async_trait]
pub trait GetExecutionPlatformsImpl: 'static + Send + Sync {
    fn get_execution_platforms_impl<'a, 'd>(
        &self,
        dice_computations: &'a mut DiceComputations<'d>,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Option<ExecutionPlatforms>>>
    where
        'd: 'a;

    async fn execution_platform_resolution_one_for_cell(
        &self,
        dice: &mut DiceComputations<'_>,
        exec_deps: Arc<[TargetLabel]>,
        toolchain_deps: Arc<[TargetConfiguredTargetLabel]>,
        exec_compatible_with: Arc<[ConfigurationSettingKey]>,
        cell: CellNameForConfigurationResolution,
    ) -> buck2_error::Result<ExecutionPlatformResolutionPartial>;

    /// Re-traverses the candidates for a target, collecting the outcome of every candidate
    /// execution platform for each of its execution groups. Used by `buck2 audit execution-platform-resolution` to reconstruct
    /// the reasons that `NoCompatiblePlatform` deliberately does not render. The per-candidate
    /// checks are DICE-cached, so for a target whose resolution just failed this mostly reads
    /// back the failed attempt.
    async fn diagnose_execution_platform_resolution(
        &self,
        dice: &mut DiceComputations<'_>,
        target: &TargetConfiguredTargetLabel,
    ) -> buck2_error::Result<ExecutionPlatformDiagnosis>;
}

pub static GET_EXECUTION_PLATFORMS: LateBinding<&'static dyn GetExecutionPlatformsImpl> =
    LateBinding::new("EXECUTION_PLATFORMS");

pub trait GetExecutionPlatforms<'d>: Send {
    /// Returns a list of the configured execution platforms. This looks up the providers on the target
    /// the `execution_platforms.target` setting names (falling back to the root cell's
    /// `build.execution_platforms` buckconfig). If there's no
    /// value configured, it will return `None` which indicates we should fallback to the legacy execution
    /// platform behavior.
    fn get_execution_platforms<'a>(
        &'a mut self,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Option<ExecutionPlatforms>>>
    where
        'd: 'a;
}

impl<'d> GetExecutionPlatforms<'d> for DiceComputations<'d> {
    fn get_execution_platforms<'a>(
        &'a mut self,
    ) -> BoxFuture<'a, buck2_error::Result<&'d Option<ExecutionPlatforms>>>
    where
        'd: 'a,
    {
        match GET_EXECUTION_PLATFORMS.get() {
            Ok(i) => i.get_execution_platforms_impl(self),
            Err(e) => futures::future::ready(Err(e)).boxed(),
        }
    }
}
