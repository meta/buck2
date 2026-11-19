/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ops::ControlFlow;
use std::sync::Arc;

use async_trait::async_trait;
use buck2_core::execution_types::executor_config::ReGangWorker;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_data::NetworkAccess;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use remote_execution as RE;

use crate::digest_config::DigestConfig;
use crate::execute::action_digest::ActionDigest;
use crate::execute::action_digest_and_blobs::ActionDigestAndBlobs;
use crate::execute::manager::CommandExecutionManager;
use crate::execute::request::CommandExecutionRequest;
use crate::execute::request::ExecutorPreference;
use crate::execute::result::CommandExecutionResult;
use crate::execute::target::CommandExecutionTarget;

pub struct PreparedAction {
    pub action_and_blobs: ActionDigestAndBlobs,
    pub platform: RE::Platform,
    pub remote_execution_dependencies: Vec<RemoteExecutorDependency>,
    pub re_gang_workers: Vec<ReGangWorker>,
    pub worker_tool_init_action: Option<ActionDigestAndBlobs>,
    pub network_access: Option<NetworkAccess>,
}

impl PreparedAction {
    pub fn digest(&self) -> ActionDigest {
        self.action_and_blobs.action.dupe()
    }
}

pub struct PreparedCommand<'a, 'b> {
    pub request: &'a CommandExecutionRequest,
    pub target: &'b dyn CommandExecutionTarget,
    pub prepared_action: &'a PreparedAction,
    pub digest_config: DigestConfig,
}

#[async_trait]
pub trait PreparedCommandExecutor: Send + Sync {
    /// Execute a command.
    ///
    /// This intentionally does not return a Result since we want to capture information about the
    /// execution even if there are errors. Any errors can be propagated by converting them
    /// to a result with CommandExecutionManager::error.
    async fn exec_cmd(
        &self,
        command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        cancellations: &CancellationContext,
    ) -> CommandExecutionResult;

    /// Checks if there is any possibility for a command with a given executor preference to
    /// be executed locally.
    fn is_local_execution_possible(&self, executor_preference: ExecutorPreference) -> bool;
}

#[async_trait]
pub trait PreparedCommandOptionalExecutor: Send + Sync {
    /// Take a command and evaluate whether it needs to be actually executed (locally or remotely) or can be skipped.
    /// In the skip case, this should handle all the things that would happen in an actual execution (materialization, output declaration, etc.).
    ///
    /// Given a command, evaluate whether the execution can be skipped. (for example because it is already cached)
    /// If it can be skipped, return a CommandExecutionResult that can be used as if the action was just executed.
    /// Otherwise, return a CommandExecutionManager that can be used to execute the action.
    async fn maybe_execute(
        &self,
        command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        cancellations: &CancellationContext,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManager>;
}

#[async_trait]
impl PreparedCommandOptionalExecutor for Arc<dyn PreparedCommandOptionalExecutor> {
    async fn maybe_execute(
        &self,
        command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        cancellations: &CancellationContext,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManager> {
        (**self)
            .maybe_execute(command, manager, cancellations)
            .await
    }
}

// When we don't want to check a command can be skipped, just use the NoOpCommandOptionalExecutor that always returns the continue case.
pub struct NoOpCommandOptionalExecutor {}

#[async_trait]
impl PreparedCommandOptionalExecutor for NoOpCommandOptionalExecutor {
    async fn maybe_execute(
        &self,
        _command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        _cancellations: &CancellationContext,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManager> {
        ControlFlow::Continue(manager)
    }
}
