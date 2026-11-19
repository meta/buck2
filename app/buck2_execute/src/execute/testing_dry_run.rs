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
use std::sync::Mutex;

use async_trait::async_trait;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_data::local_command::EnvironmentRule;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::time_span::TimeSpan;
use dice_futures::cancellation::CancellationContext;

use crate::artifact_value::ArtifactValue;
use crate::execute::action_digest::ActionDigest;
use crate::execute::kind::CommandExecutionKind;
use crate::execute::manager::CommandExecutionManager;
use crate::execute::manager::CommandExecutionManagerExt;
use crate::execute::prepared::PreparedCommand;
use crate::execute::prepared::PreparedCommandExecutor;
use crate::execute::request::CommandExecutionOutput;
use crate::execute::request::ExecutorPreference;
use crate::execute::result::CommandExecutionMetadata;
use crate::execute::result::CommandExecutionResult;

#[derive(Debug, PartialEq, Eq)]
pub struct DryRunEntry {
    pub args: Vec<String>,
    pub outputs: Vec<CommandExecutionOutput>,
    pub env: BuckFixedSortedMap<String, String>,
}

/// Records executed commands into the provided tracker and returns a successful result for all commands.
/// If the filesystem is supplied, the dry run executor will write the executed command as contents
/// to the output file.
pub struct DryRunExecutor {
    tracker: Arc<Mutex<Vec<DryRunEntry>>>,
    fs: ArtifactFs,
}

impl DryRunExecutor {
    pub fn new(tracker: Arc<Mutex<Vec<DryRunEntry>>>, fs: ArtifactFs) -> Self {
        Self { tracker, fs }
    }
}

#[async_trait]
impl PreparedCommandExecutor for DryRunExecutor {
    async fn exec_cmd(
        &self,
        command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        _cancellations: &CancellationContext,
    ) -> CommandExecutionResult {
        let PreparedCommand {
            request,
            target: _target,
            prepared_action: _prepared_action,
            digest_config,
        } = command;

        let manager = manager.claim().await;

        let args = request.all_args_vec();
        let outputs = request.outputs().map(|o| o.cloned()).collect();
        let env = request.env().to_owned();

        self.tracker
            .lock()
            .unwrap()
            .push(DryRunEntry { args, outputs, env });

        let exec_kind = CommandExecutionKind::Local {
            digest: ActionDigest::empty(digest_config.cas_digest_config()),
            command: Default::default(),
            env: Default::default(),
            working_directory: request.working_directory().to_owned(),
            environment_rule: EnvironmentRule::Unspecified,
        };

        match request
            .outputs()
            .map(|x| {
                let path = x.resolve(&self.fs, None)?.into_path();
                self.fs.fs().write_file(&path, "", false)?;
                Ok((x.cloned(), ArtifactValue::file(digest_config.empty_file())))
            })
            .collect::<buck2_error::Result<_>>()
        {
            Ok(outputs) => manager.success(
                exec_kind,
                outputs,
                Default::default(),
                CommandExecutionMetadata::empty(TimeSpan::empty_now()),
            ),
            // NOTE: This should probably be an error() but who cares.
            Err(..) => manager.failure(
                exec_kind,
                BuckMutInsertionMap::default(),
                Default::default(),
                Some(1),
                CommandExecutionMetadata::empty(TimeSpan::empty_now()),
                None,
                false,
            ),
        }
    }

    fn is_local_execution_possible(&self, _executor_preference: ExecutorPreference) -> bool {
        false
    }
}
