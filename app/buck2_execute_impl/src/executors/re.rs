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
use std::time::Duration;

use async_trait::async_trait;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::ReGangWorker;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::execution_types::revision::LazyVcsRevision;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_core::soft_error_or_panic;
use buck2_events::dispatch::span_async;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::action_digest::ActionDigest;
use buck2_execute::execute::blobs::ActionBlobs;
use buck2_execute::execute::kind::CommandExecutionKind;
use buck2_execute::execute::kind::RemoteCommandExecutionDetails;
use buck2_execute::execute::kind::RemoteExecutionOrigin;
use buck2_execute::execute::manager::CommandExecutionManager;
use buck2_execute::execute::manager::CommandExecutionManagerExt;
use buck2_execute::execute::output::CommandStdStreams;
use buck2_execute::execute::prepared::PreparedAction;
use buck2_execute::execute::prepared::PreparedCommand;
use buck2_execute::execute::prepared::PreparedCommandExecutor;
use buck2_execute::execute::request::CommandExecutionPaths;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::request::ExecutorPreference;
use buck2_execute::execute::result::CommandCancellationReason;
use buck2_execute::execute::result::CommandExecutionErrorType;
use buck2_execute::execute::result::CommandExecutionMetadata;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::knobs::ExecutorGlobalKnobs;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::re::action_identity::ReActionIdentity;
use buck2_execute::re::client::CancellationReason;
use buck2_execute::re::client::ExecuteResponseOrCancelled;
use buck2_execute::re::error::RemoteExecutionError;
use buck2_execute::re::error::get_re_error_tag;
use buck2_execute::re::manager::ManagedRemoteExecutionClient;
use buck2_execute::re::output_trees_download_config::OutputTreesDownloadConfig;
use buck2_execute::re::remote_action_result::ExecuteResponseWithQueueStats;
use buck2_execute::re::remote_action_result::RemoteActionResult;
use buck2_hash::BuckMutInsertionMap;
use buck2_util::time_span::TimeSpan;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use itertools::Either;
use remote_execution as RE;
use remote_execution::TCode;
use remote_execution::TCodeReasonGroup;
use tracing::info;

use crate::incremental_actions_helper::save_content_based_incremental_state;
use crate::re::download::DownloadResult;
use crate::re::download::download_action_results;
use crate::re::paranoid_download::ParanoidDownloader;
use crate::sqlite::incremental_state_db::IncrementalDbState;
use crate::storage_resource_exhausted::is_storage_resource_exhausted;

#[derive(Debug, buck2_error::Error)]
pub enum RemoteExecutorError {
    #[error("Trying to execute a `local_only = True` action on remote executor")]
    #[buck2(input)]
    LocalOnlyAction,
}

pub struct ReExecutor {
    pub artifact_fs: ArtifactFs,
    pub project_fs: ProjectRoot,
    pub revision: Arc<LazyVcsRevision>,
    pub materializer: Arc<dyn Materializer>,
    pub incremental_db_state: Arc<IncrementalDbState>,
    pub re_client: ManagedRemoteExecutionClient,
    /// For what the executor materializes; the RE request itself runs under `re_client`'s.
    pub invocation_re_use_case: RemoteExecutorUseCase,
    pub re_action_key: Option<String>,
    pub knobs: ExecutorGlobalKnobs,
    pub skip_cache_read: bool,
    pub skip_cache_write: bool,
    pub re_max_queue_time: Option<Duration>,
    pub re_resource_units: Option<i64>,
    pub paranoid: Option<ParanoidDownloader>,
    pub materialize_failed_inputs: bool,
    pub materialize_failed_outputs: bool,
    pub dependencies: Vec<RemoteExecutorDependency>,
    pub gang_workers: Vec<ReGangWorker>,
    pub output_trees_download_config: OutputTreesDownloadConfig,
    pub priority: Option<i32>,
}

impl ReExecutor {
    async fn upload(
        &self,
        manager: CommandExecutionManager,
        identity: &ReActionIdentity<'_>,
        blobs: &ActionBlobs,
        paths: &CommandExecutionPaths,
    ) -> ControlFlow<CommandExecutionResult, CommandExecutionManager> {
        let re_client = &self.re_client;

        let upload_response = span_async(buck2_data::ReUploadStart {}, async move {
            let res = re_client
                .upload(
                    &self.project_fs,
                    self.materializer.as_ref(),
                    blobs,
                    ProjectRelativePath::empty(),
                    paths.input_directory(),
                    Some(identity),
                )
                .await;
            match res {
                Ok(stats) => (
                    Ok(()),
                    buck2_data::ReUploadEnd {
                        digests_uploaded: Some(stats.total.digests_uploaded),
                        bytes_uploaded: Some(stats.total.bytes_uploaded),
                        stats_by_extension: stats.by_extension,
                        stats: None,
                    },
                ),
                Err(e) => (Err(e), buck2_data::ReUploadEnd::default()),
            }
        })
        .await;

        match upload_response {
            Ok(()) => {}
            Err(e) => {
                let e: buck2_error::Error = e;
                let is_storage_resource_exhausted = e
                    .find_typed_context::<RemoteExecutionError>()
                    .is_some_and(|re_client_error| {
                        is_storage_resource_exhausted(re_client_error.as_ref())
                    });
                let error_type = if is_storage_resource_exhausted {
                    CommandExecutionErrorType::StorageResourceExhausted
                } else {
                    CommandExecutionErrorType::Other
                };
                return ControlFlow::Break(manager.error_classified(
                    "remote_upload_error",
                    e,
                    error_type,
                ));
            }
        };

        ControlFlow::Continue(manager)
    }

    async fn re_execute<'a>(
        &self,
        mut manager: CommandExecutionManager,
        identity: &ReActionIdentity<'_>,
        request: &CommandExecutionRequest,
        action_digest: &ActionDigest,
        digest_config: DigestConfig,
        platform: &RE::Platform,
        dependencies: impl IntoIterator<Item = &'a RemoteExecutorDependency>,
        re_gang_workers: &[buck2_core::execution_types::executor_config::ReGangWorker],
        meta_internal_extra_params: &MetaInternalExtraParams,
        worker_tool_action_digest: Option<ActionDigest>,
    ) -> ControlFlow<CommandExecutionResult, (CommandExecutionManager, ExecuteResponseWithQueueStats)>
    {
        info!(
            "RE command line:\n```\n$ {}\n```\n for action `{}`",
            request.all_args_str(),
            action_digest,
        );

        let now = TimeSpan::start_now();

        let execute_response_fut = self.re_client.execute(
            action_digest.dupe(),
            platform,
            dependencies,
            re_gang_workers,
            identity,
            &mut manager,
            self.skip_cache_read || request.induced_cache_miss(),
            self.skip_cache_write,
            self.re_max_queue_time,
            self.re_resource_units,
            &self.knobs,
            meta_internal_extra_params,
            worker_tool_action_digest,
            self.priority,
        );

        let execute_response =
            if let Some(timeout) = buck2_common::self_test_timeout::maybe_cap_timeout(None) {
                match tokio::time::timeout(timeout, execute_response_fut).await {
                    Ok(resp) => resp,
                    Err(_) => {
                        return ControlFlow::Break(manager.error(
                            "re_timeout_exceeded",
                            buck2_error::buck2_error!(
                                buck2_error::ErrorTag::Tier0,
                                "Command {} exceeded its timeout (timeout was {}s)",
                                &identity.action_key,
                                timeout.as_secs(),
                            ),
                        ));
                    }
                }
            } else {
                execute_response_fut.await
            };

        let remote_details = RemoteCommandExecutionDetails::new(
            action_digest.dupe(),
            None,
            self.re_client.get_session_id().await.ok(),
            self.re_client.use_case,
            platform,
            worker_tool_action_digest.is_some(),
        );

        let response =
            match execute_response {
                Ok(ExecuteResponseOrCancelled::Response(result)) => result,
                Ok(ExecuteResponseOrCancelled::Cancelled(cancelled, queue_stats, _)) => {
                    let reason = cancelled.reason.map_or(
                        CommandCancellationReason::NotSpecified,
                        |reason| match reason {
                            CancellationReason::NotSpecified => {
                                CommandCancellationReason::NotSpecified
                            }
                            CancellationReason::ReQueueTimeout => {
                                CommandCancellationReason::ReQueueTimeout
                            }
                        },
                    );
                    return ControlFlow::Break(manager.cancel(
                        CommandExecutionKind::Remote {
                            details: remote_details,
                            queue_time: queue_stats.cumulative_queue_duration,
                            materialized_inputs_for_failed: None,
                            materialized_outputs_for_failed_actions: None,
                            origin: RemoteExecutionOrigin::Executed,
                        },
                        reason,
                        CommandExecutionMetadata {
                            queue_duration: Some(queue_stats.cumulative_queue_duration),
                            ..CommandExecutionMetadata::empty(TimeSpan::empty_now())
                        },
                    ));
                }
                Err(e) => {
                    if is_re_queue_full(&e) {
                        return ControlFlow::Break(manager.cancel(
                            CommandExecutionKind::Remote {
                                details: remote_details,
                                queue_time: Duration::ZERO,
                                materialized_inputs_for_failed: None,
                                materialized_outputs_for_failed_actions: None,
                                origin: RemoteExecutionOrigin::Executed,
                            },
                            CommandCancellationReason::ReQueueTimeout,
                            CommandExecutionMetadata::empty(TimeSpan::empty_now()),
                        ));
                    }
                    return ControlFlow::Break(manager.error("remote_call_error", e));
                }
            };

        let execution_kind = response.execution_kind(remote_details);
        let manager = manager.with_execution_kind(execution_kind.clone());
        let additional_message = if response.execute_response.status.message.is_empty() {
            None
        } else {
            Some(response.execute_response.status.message.clone())
        };

        if response.execute_response.status.code != TCode::OK {
            let res = if let Some(out) = as_missing_outputs_error(&response.execute_response.status)
            {
                // TODO: Add a dedicated report variant for this.
                // NOTE: We don't get stdout / stderr from RE when this happens, so the best we can
                // do here is just pass on the error.
                manager.failure(
                    execution_kind,
                    BuckMutInsertionMap::default(),
                    CommandStdStreams::Local {
                        stdout: Vec::new(),
                        stderr: out.to_owned().into(),
                    },
                    // We also don't get this output so don't put trash in here.
                    None,
                    CommandExecutionMetadata::empty(TimeSpan::empty_now()),
                    additional_message,
                    false,
                )
            } else if is_timeout_error(&response.execute_response.status)
                && request.timeout().is_some()
            {
                manager.timeout(
                    execution_kind,
                    BuckMutInsertionMap::default(),
                    // Checked above: we fallthrough to the error path if we didn't set a timeout
                    // and yet received one.
                    request.timeout().unwrap(),
                    CommandStdStreams::Remote(response.std_streams(&self.re_client, digest_config)),
                    CommandExecutionMetadata::from_re_timing(response.timing(), now.end_now()),
                    additional_message,
                )
            } else {
                let error_type = if is_storage_resource_exhausted(&response.execute_response.status)
                {
                    CommandExecutionErrorType::StorageResourceExhausted
                } else {
                    CommandExecutionErrorType::Other
                };
                manager.error_classified(
                    "remote_exec_error",
                    ReErrorWrapper {
                        action_digest: action_digest.dupe(),
                        inner: response.execute_response.status,
                    },
                    error_type,
                )
            };

            return ControlFlow::Break(res);
        }

        if let Some(timeout) = request.timeout() {
            let execution_time = response.timing().execution_time;

            if execution_time > timeout {
                soft_error_or_panic!(
                    "re_timeout_exceeded",
                    buck2_error::buck2_error!(
                        buck2_error::ErrorTag::Tier0,
                        "Command {} exceeded its timeout (ran for {}s, timeout was {}s)",
                        &identity.action_key,
                        execution_time.as_secs(),
                        timeout.as_secs(),
                    )
                );
            }
        }

        ControlFlow::Continue((manager, response))
    }
}

#[async_trait]
impl PreparedCommandExecutor for ReExecutor {
    async fn exec_cmd(
        &self,
        command: &PreparedCommand<'_, '_>,
        manager: CommandExecutionManager,
        cancellations: &CancellationContext,
    ) -> CommandExecutionResult {
        let PreparedCommand {
            request,
            target,
            prepared_action:
                PreparedAction {
                    action_and_blobs,
                    platform,
                    remote_execution_dependencies,
                    re_gang_workers,
                    worker_tool_init_action,
                    network_access: _,
                },
            digest_config,
        } = command;

        let details = RemoteCommandExecutionDetails::new(
            command.prepared_action.digest(),
            command.request.remote_dep_file_key,
            self.re_client.get_session_id().await.ok(),
            self.re_client.use_case,
            platform,
            request.remote_worker().is_some() && worker_tool_init_action.is_some(),
        );
        let manager = manager.with_execution_kind(CommandExecutionKind::Remote {
            details: details.clone(),
            queue_time: Duration::ZERO,
            materialized_inputs_for_failed: None,
            materialized_outputs_for_failed_actions: None,
            origin: RemoteExecutionOrigin::Executed,
        });

        if command.request.executor_preference().requires_local() {
            return ControlFlow::Break(
                manager.error("remote_prepare", RemoteExecutorError::LocalOnlyAction),
            )?;
        }

        let identity = ReActionIdentity::new(
            *target,
            self.re_action_key.as_deref(),
            request.paths(),
            Some(action_and_blobs.action.raw_digest().to_string()),
        );

        // TODO(bobyf, torozco): remote execution probably needs to explicitly handle cancellations
        let manager = self
            .upload(manager, &identity, &action_and_blobs.blobs, request.paths())
            .await?;

        let manager = if let (Some(worker), Some(worker_tool_init_action)) =
            (request.remote_worker(), worker_tool_init_action)
        {
            self.upload(
                manager,
                &identity,
                &worker_tool_init_action.blobs,
                &worker.input_paths,
            )
            .await?
        } else {
            manager
        };
        let worker_tool_action_digest = worker_tool_init_action.clone().map(|w| w.action);

        let execution_time = TimeSpan::start_now();

        let re_gang_workers: Vec<_> = self
            .gang_workers
            .iter()
            .chain(re_gang_workers.iter())
            .cloned()
            .collect();
        let remote_execution_dependencies = self
            .dependencies
            .iter()
            .chain(remote_execution_dependencies.iter());
        // Avoid starting a VCS subprocess unless interpolation requires it. `re_execute` borrows
        // its dependencies, so keep any interpolated clones alive until that call completes.
        let resolved_remote_execution_dependencies = if remote_execution_dependencies
            .clone()
            .any(RemoteExecutorDependency::requires_revision)
        {
            let revision = self.revision.get().await;
            Some(
                remote_execution_dependencies
                    .clone()
                    .map(|dependency| dependency.with_revision(revision))
                    .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        let remote_execution_dependencies = match &resolved_remote_execution_dependencies {
            Some(dependencies) => Either::Left(dependencies.iter()),
            None => Either::Right(remote_execution_dependencies),
        };

        let (manager, response) = self
            .re_execute(
                manager,
                &identity,
                request,
                &action_and_blobs.action,
                *digest_config,
                platform,
                remote_execution_dependencies,
                &re_gang_workers,
                command.request.meta_internal_extra_params(),
                worker_tool_action_digest,
            )
            .await?;

        let exit_code = response.execute_response.action_result.exit_code;
        let additional_message = if response.execute_response.status.message.is_empty() {
            None
        } else {
            Some(response.execute_response.status.message.clone())
        };

        let res = download_action_results(
            request,
            execution_time,
            &*self.materializer,
            &self.re_client,
            self.invocation_re_use_case,
            *digest_config,
            manager,
            &identity,
            buck2_data::ReStage {
                stage: Some(buck2_data::ReDownload {}.into()),
            }
            .into(),
            request.paths(),
            request.outputs(),
            details,
            &response,
            self.paranoid.as_ref(),
            cancellations,
            exit_code,
            &self.artifact_fs,
            self.materialize_failed_inputs,
            self.materialize_failed_outputs,
            additional_message,
            &self.output_trees_download_config,
        )
        .boxed()
        .await;

        let DownloadResult::Result(mut res) = res;
        res.action_result = Some(response.execute_response.action_result);

        if let Some(run_action_key) = request.run_action_key()
            && !request.outputs_cleanup
        {
            save_content_based_incremental_state(
                run_action_key.clone(),
                &self.incremental_db_state,
                &self.artifact_fs,
                &res,
            );
        }

        res
    }

    fn is_local_execution_possible(&self, _executor_preference: ExecutorPreference) -> bool {
        false
    }
}

#[derive(buck2_error::Error, Debug)]
#[error(
    "action_digest={}, re_code={}, re_message={}",
    action_digest,
    inner.code,
    inner.message
)]
#[buck2(tag = get_re_error_tag(&inner.code))]
struct ReErrorWrapper {
    action_digest: ActionDigest,
    inner: remote_execution::TStatus,
}

fn as_missing_outputs_error(err: &remote_execution::TStatus) -> Option<&str> {
    // A dedicated error code would be better for this :(
    if err.message.contains("OUTMISS") {
        Some(&err.message)
    } else {
        None
    }
}

fn is_timeout_error(err: &remote_execution::TStatus) -> bool {
    #[cfg(fbcode_build)]
    {
        // Not ideal, but DEADLINE_EXCEEDED will show up if you e.g. timeout connecting to RE, so we
        // need to actually match on the message :(
        err.code == TCode::DEADLINE_EXCEEDED && err.message.contains("Execution timed out")
    }

    #[cfg(not(fbcode_build))]
    {
        // Not obvious what this looks like in the GRPC API.
        let _ignored = err;
        false
    }
}

fn is_re_queue_full(e: &buck2_error::Error) -> bool {
    #[cfg(all(fbcode_build, target_os = "linux"))]
    let enabled = justknobs::eval(
        "buck2/remote_execution:re_queue_full_as_cancelled",
        None,
        None,
    )
    .unwrap_or(false);

    #[cfg(not(all(fbcode_build, target_os = "linux")))]
    let enabled = true;

    if !enabled {
        return false;
    }

    e.find_typed_context::<RemoteExecutionError>()
        .is_some_and(|re_err| re_err.group == TCodeReasonGroup::USER_QUEUE_FULL)
}
