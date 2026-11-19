/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Attaching command execution related data to Dice

use std::sync::Arc;

use buck2_core::execution_types::executor_config::CommandExecutorConfig;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_error::BuckErrorContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_execute::execute::cache_uploader::UploadCache;
use buck2_execute::execute::low_pass_filter::LowPassFilter;
use buck2_execute::execute::prepared::PreparedCommandExecutor;
use buck2_execute::execute::prepared::PreparedCommandOptionalExecutor;
use buck2_execute::execute::write_action_cache::WriteActionCache;
use buck2_execute::re::manager::UnconfiguredRemoteExecutionClient;
use buck2_execute::re::output_trees_download_config::OutputTreesDownloadConfig;
use dice::DiceComputations;
use dice::DiceData;
use dice::DiceDataBuilder;
use dice::UserComputationData;
use dupe::Dupe;
use remote_execution as RE;

pub struct CommandExecutorResponse {
    pub executor: Arc<dyn PreparedCommandExecutor>,
    pub platform: RE::Platform,
    pub action_cache_checker: Arc<dyn PreparedCommandOptionalExecutor>,
    pub remote_dep_file_cache_checker: Arc<dyn PreparedCommandOptionalExecutor>,
    pub cache_uploader: Arc<dyn UploadCache>,
    pub output_trees_download_config: OutputTreesDownloadConfig,
    /// The action cache as write actions use it; `None` where the executor reads no cache.
    pub write_action_cache: Option<Arc<dyn WriteActionCache>>,
}

pub trait SetCommandExecutor {
    fn set_command_executor(&mut self, init: Box<dyn HasCommandExecutor + Send + Sync + 'static>);
}

pub trait HasCommandExecutor {
    fn get_command_executor(
        &self,
        artifact_fs: &ArtifactFs,
        config: &CommandExecutorConfig,
    ) -> buck2_error::Result<CommandExecutorResponse>;
}

impl SetCommandExecutor for UserComputationData {
    fn set_command_executor(
        &mut self,
        delegate: Box<dyn HasCommandExecutor + Send + Sync + 'static>,
    ) {
        self.data.set(HasCommandExecutorHolder { delegate })
    }
}

pub trait DiceHasCommandExecutor {
    fn get_command_executor_from_dice(
        &self,
        artifact_fs: &ArtifactFs,
        config: &CommandExecutorConfig,
    ) -> buck2_error::Result<CommandExecutorResponse>;
}

impl DiceHasCommandExecutor for DiceComputations<'_> {
    fn get_command_executor_from_dice(
        &self,
        artifact_fs: &ArtifactFs,
        config: &CommandExecutorConfig,
    ) -> buck2_error::Result<CommandExecutorResponse> {
        let holder = self
            .per_transaction_data()
            .data
            .get::<HasCommandExecutorHolder>()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .buck_error_context("CommandExecutorDelegate should be set")?;
        holder.delegate.get_command_executor(artifact_fs, config)
    }
}

struct HasCommandExecutorHolder {
    delegate: Box<dyn HasCommandExecutor + Send + Sync + 'static>,
}

pub trait HasFallbackExecutorConfig {
    fn get_fallback_executor_config(&self) -> &Arc<CommandExecutorConfig>;
}

impl HasFallbackExecutorConfig for DiceComputations<'_> {
    fn get_fallback_executor_config(&self) -> &Arc<CommandExecutorConfig> {
        self.per_transaction_data()
            .data
            .get::<Arc<CommandExecutorConfig>>()
            .expect("CommandExecutorConfig should be set")
    }
}

pub fn set_fallback_executor_config(data: &mut DiceData, config: Arc<CommandExecutorConfig>) {
    data.set(config)
}

pub trait SetLowPassFilter {
    fn set_low_pass_filter(&mut self, low_pass_filter: Arc<LowPassFilter>);
}

pub trait HasLowPassFilter {
    /// The command's gate on local work, which the hybrid executor runs local commands behind
    /// and a write action writes locally behind.
    fn get_low_pass_filter(&self) -> &LowPassFilter;
}

impl SetLowPassFilter for UserComputationData {
    fn set_low_pass_filter(&mut self, low_pass_filter: Arc<LowPassFilter>) {
        self.data.set(low_pass_filter);
    }
}

impl HasLowPassFilter for UserComputationData {
    fn get_low_pass_filter(&self) -> &LowPassFilter {
        self.data
            .get::<Arc<LowPassFilter>>()
            .expect("LowPassFilter should be set")
    }
}

pub trait SetReClient {
    fn set_re_client(&mut self, re_client: UnconfiguredRemoteExecutionClient);
}

pub trait GetReClient {
    fn get_re_client(&self) -> &UnconfiguredRemoteExecutionClient;
}

impl SetReClient for UserComputationData {
    fn set_re_client(&mut self, re_client: UnconfiguredRemoteExecutionClient) {
        self.data.set(re_client);
    }
}

impl GetReClient for UserComputationData {
    fn get_re_client(&self) -> &UnconfiguredRemoteExecutionClient {
        self.data
            .get::<UnconfiguredRemoteExecutionClient>()
            .expect("RE client should be set")
    }
}

#[derive(Debug, Clone, Copy, Dupe)]
pub struct InvalidationTrackingConfig {
    pub enabled: bool,
}

pub trait SetInvalidationTrackingConfig {
    fn set_invalidation_tracking_config(&mut self, enabled: bool);
}

pub trait GetInvalidationTrackingConfig {
    fn get_invalidation_tracking_config(&self) -> InvalidationTrackingConfig;
}

impl SetInvalidationTrackingConfig for DiceDataBuilder {
    fn set_invalidation_tracking_config(&mut self, enabled: bool) {
        self.set(InvalidationTrackingConfig { enabled });
    }
}

impl SetInvalidationTrackingConfig for DiceData {
    fn set_invalidation_tracking_config(&mut self, enabled: bool) {
        self.set(InvalidationTrackingConfig { enabled });
    }
}

impl GetInvalidationTrackingConfig for DiceComputations<'_> {
    fn get_invalidation_tracking_config(&self) -> InvalidationTrackingConfig {
        *self
            .global_data()
            .get::<InvalidationTrackingConfig>()
            .expect("InvalidationTrackingConfig should be set")
    }
}
