/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::convert::Infallible;
use std::ops::ControlFlow;
use std::ops::FromResidual;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use buck2_common::file_ops::metadata::FileDigest;
use buck2_common::file_ops::metadata::FileMetadata;
use buck2_common::file_ops::metadata::Symlink;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::artifact_path_resolver::ArtifactFs;
use buck2_core::fs::buck_out_path::BuildArtifactPath;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_data::timing::Micros;
use buck2_directory::directory::entry::DirectoryEntry;
use buck2_error::BuckErrorContext;
use buck2_events::dispatch::console_message;
use buck2_events::dispatch::span_async;
use buck2_execute::artifact_value::ArtifactValue;
use buck2_execute::digest::CasDigestFromReExt;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::directory::ActionDirectoryMember;
use buck2_execute::directory::extract_artifact_value;
use buck2_execute::directory::re_tree_to_directory;
use buck2_execute::execute::executor_stage_async;
use buck2_execute::execute::kind::RemoteCommandExecutionDetails;
use buck2_execute::execute::manager::CommandExecutionManager;
use buck2_execute::execute::manager::CommandExecutionManagerExt;
use buck2_execute::execute::manager::CommandExecutionManagerWithClaim;
use buck2_execute::execute::output::CommandStdStreams;
use buck2_execute::execute::request::CommandExecutionOutput;
use buck2_execute::execute::request::CommandExecutionOutputRef;
use buck2_execute::execute::request::CommandExecutionPaths;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::result::CommandExecutionErrorType;
use buck2_execute::execute::result::CommandExecutionMetadata;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute::materialize::materializer::CasDownloadInfo;
use buck2_execute::materialize::materializer::DeclareArtifactPayload;
use buck2_execute::materialize::materializer::MaterializationPurpose;
use buck2_execute::materialize::materializer::MaterializeRequest;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::re::action_identity::ReActionIdentity;
use buck2_execute::re::error::RemoteExecutionError;
use buck2_execute::re::manager::ManagedRemoteExecutionClient;
use buck2_execute::re::output_trees_download_config::OutputTreesDownloadConfig;
use buck2_execute::re::remote_action_result::RemoteActionResult;
use buck2_execute::re::ttl::re_expiration_from_ttl;
use buck2_fs::paths::RelativePathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutSet;
use buck2_util::time_span::TimeSpan;
use buck2_util::time_span::TimeSpanBuilder;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use futures::FutureExt;
use futures::future;
use jiff::Timestamp;
use remote_execution as RE;

use crate::executors::local::materialize_inputs;
use crate::re::paranoid_download::ParanoidDownloader;
use crate::storage_resource_exhausted::is_storage_resource_exhausted;

pub async fn download_action_results<'a>(
    request: &CommandExecutionRequest,
    execution_time: TimeSpanBuilder,
    materializer: &dyn Materializer,
    re_client: &ManagedRemoteExecutionClient,
    invocation_re_use_case: RemoteExecutorUseCase,
    digest_config: DigestConfig,
    manager: CommandExecutionManager,
    identity: &ReActionIdentity<'_>,
    stage: buck2_data::executor_stage_start::Stage,
    paths: &CommandExecutionPaths,
    requested_outputs: impl IntoIterator<Item = CommandExecutionOutputRef<'a>>,
    details: RemoteCommandExecutionDetails,
    response: &dyn RemoteActionResult,
    paranoid: Option<&ParanoidDownloader>,
    cancellations: &CancellationContext,
    action_exit_code: i32,
    artifact_fs: &ArtifactFs,
    materialize_failed_re_action_inputs: bool,
    materialize_failed_re_action_outputs: bool,
    additional_message: Option<String>,
    output_trees_download_config: &OutputTreesDownloadConfig,
) -> DownloadResult {
    let std_streams = response.std_streams(re_client, digest_config);
    let std_streams = async {
        if request.prefetch_lossy_stderr() {
            std_streams.prefetch_lossy_stderr().await
        } else {
            std_streams
        }
    };

    if action_exit_code != 0 && manager.inner.intend_to_fallback_on_failure {
        // Do not attempt to download outputs in this case so
        // as to avoid cancelling in-flight local execution:
        // either local already finished and the outputs are
        // already there, or local hasn't finished, and then
        // it will produce outputs.

        let std_streams = std_streams.await;
        return DownloadResult::Result(manager.failure(
            response.execution_kind(details),
            BuckMutInsertionMap::default(),
            CommandStdStreams::Remote(std_streams),
            Some(action_exit_code),
            CommandExecutionMetadata::from_re_timing(response.timing(), TimeSpan::empty_now()),
            additional_message,
            false,
        ));
    }
    let downloader = CasDownloader {
        materializer,
        re_client,
        digest_config,
        paranoid,
        output_trees_download_config,
    };

    let download = downloader.download(
        artifact_fs,
        manager,
        identity,
        stage,
        paths,
        requested_outputs,
        response,
        &details,
        cancellations,
    );

    let (download, std_streams) = future::join(download, std_streams).await;
    let (manager, outputs) = download?;

    let res = match action_exit_code {
        0 => manager.success(
            response.execution_kind(details),
            outputs,
            CommandStdStreams::Remote(std_streams),
            CommandExecutionMetadata::from_re_timing(response.timing(), execution_time.end_now()),
        ),
        e => {
            let materialized_inputs = if materialize_failed_re_action_inputs {
                executor_stage_async(
                    buck2_data::ReStage {
                        stage: Some(buck2_data::MaterializeFailedInputs {}.into()),
                    },
                    async move {
                        match materialize_inputs(
                            artifact_fs,
                            materializer,
                            request,
                            digest_config,
                            invocation_re_use_case,
                            Vec::new(),
                        )
                        .await
                        {
                            Ok(materialized_paths) => Some(materialized_paths.paths.clone()),
                            Err(e) => {
                                // TODO(minglunli): Properly handle this and the error below and add a test for it.
                                console_message(format!(
                                    "Failed to materialize inputs for failed action: {e}"
                                ));
                                None
                            }
                        }
                    },
                )
                .await
            } else {
                None
            };

            let materialized_outputs = match materialize_failed_build_outputs(
                artifact_fs,
                materializer,
                request,
                &outputs,
                materialize_failed_re_action_outputs,
                invocation_re_use_case,
            )
            .await
            {
                Ok(materialized_paths) => Some(materialized_paths.clone()),
                Err(e) => {
                    console_message(format!(
                        "Failed to materialize outputs for failed action: {e}"
                    ));
                    None
                }
            };

            manager.failure(
                response.execution_kind_for_failed_actions(
                    details,
                    materialized_inputs,
                    materialized_outputs,
                ),
                outputs,
                CommandStdStreams::Remote(std_streams),
                Some(e),
                CommandExecutionMetadata::from_re_timing(
                    response.timing(),
                    execution_time.end_now(),
                ),
                additional_message,
                false,
            )
        }
    };

    DownloadResult::Result(res)
}

async fn materialize_failed_build_outputs(
    artifact_fs: &ArtifactFs,
    materializer: &dyn Materializer,
    request: &CommandExecutionRequest,
    available_outputs: &BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>,
    materialize_failed_re_action_outputs: bool,
    re_use_case: RemoteExecutorUseCase,
) -> buck2_error::Result<Vec<ProjectRelativePathBuf>> {
    let mut paths = vec![];
    if !materialize_failed_re_action_outputs && request.outputs_for_error_handler().is_empty() {
        // Nothing to materialize
        return Ok(paths);
    }

    let materialize_select_outputs: BuckMutSet<&BuildArtifactPath> =
        request.outputs_for_error_handler().iter().collect();

    let mut artifacts = Vec::new();
    for output in request.outputs() {
        if let CommandExecutionOutputRef::BuildArtifact { path, .. } = output {
            // If materialize_failed_re_action_outputs is not set and materialize_select_outputs is not empty,
            // only materialize outputs in the set. Otherwise, materialize all outputs.
            if !materialize_failed_re_action_outputs
                && !materialize_select_outputs.is_empty()
                && !materialize_select_outputs.contains(path)
            {
                continue;
            }

            let value = available_outputs.get(&output.cloned());
            let content_hash = value.and_then(|value| {
                if path.is_content_based_path() {
                    Some(value.content_based_path_hash())
                } else {
                    None
                }
            });

            let resolved = artifact_fs.resolve_build(path, content_hash.as_ref())?;
            // An output the failed run did not produce has nothing to materialize. Its path is
            // still reported, and the error handler decides what its absence means.
            if let Some(value) = value {
                artifacts.push((resolved.clone(), value.dupe()));
            }
            paths.push(resolved);
        }
    }

    if !artifacts.is_empty() {
        let response = materializer
            .materialize(MaterializeRequest {
                artifacts,
                outputs: Vec::new(),
                purpose: MaterializationPurpose::IntermediateOnly,
                re_use_case,
            })
            .await?;
        for result in response.results {
            result?;
        }
        // These are read after the command, by a human or by whatever reads the paths the event
        // names, which no lease can cover; the error handler reads its outputs under one of its
        // own (`materialize_outputs_for_error_handler`).
        drop(response.lease);
    }

    Ok(paths)
}

pub struct CasDownloader<'a> {
    pub materializer: &'a dyn Materializer,
    pub re_client: &'a ManagedRemoteExecutionClient,
    pub digest_config: DigestConfig,
    pub paranoid: Option<&'a ParanoidDownloader>,
    pub output_trees_download_config: &'a OutputTreesDownloadConfig,
}

/// Where a download's time went. Measured for every download; reported for cache hits. A phase
/// the download did not reach, because it failed earlier, stays `None`.
#[derive(Default)]
struct DownloadPhases {
    tree_admission: Option<Duration>,
    tree_fetch: Option<Duration>,
    local_bookkeeping: Option<Duration>,
    declare: Option<Duration>,
    tree_count: u64,
}

impl DownloadPhases {
    fn to_proto(&self) -> buck2_data::CacheHitTimings {
        let us = |d: Option<Duration>| d.map(|d| Micros::from(d).into());
        buck2_data::CacheHitTimings {
            tree_admission_us: us(self.tree_admission),
            tree_fetch_us: us(self.tree_fetch),
            local_bookkeeping_us: us(self.local_bookkeeping),
            declare_us: us(self.declare),
            tree_count: self.tree_count,
        }
    }
}

impl CasDownloader<'_> {
    async fn download<'a>(
        &self,
        artifact_fs: &ArtifactFs,
        manager: CommandExecutionManager,
        identity: &ReActionIdentity<'_>,
        stage: buck2_data::executor_stage_start::Stage,
        paths: &CommandExecutionPaths,
        requested_outputs: impl IntoIterator<Item = CommandExecutionOutputRef<'a>>,
        output_spec: &dyn RemoteActionResult,
        details: &RemoteCommandExecutionDetails,
        cancellations: &CancellationContext,
    ) -> ControlFlow<
        DownloadResult,
        (
            CommandExecutionManagerWithClaim,
            BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>,
        ),
    > {
        let manager = manager.with_execution_kind(output_spec.execution_kind(details.clone()));
        let is_cache_hit = matches!(stage, buck2_data::executor_stage_start::Stage::CacheHit(_));
        span_async(
            buck2_data::ExecutorStageStart { stage: Some(stage) },
            async {
                let mut phases = DownloadPhases::default();
                let result = self
                    .download_inner(
                        artifact_fs,
                        manager,
                        identity,
                        paths,
                        requested_outputs,
                        output_spec,
                        details,
                        cancellations,
                        &mut phases,
                    )
                    .await;
                let end = buck2_data::ExecutorStageEnd {
                    cache_hit: is_cache_hit.then(|| phases.to_proto()),
                    ..Default::default()
                };
                (result, end)
            },
        )
        .await
    }

    async fn download_inner<'a>(
        &self,
        artifact_fs: &ArtifactFs,
        manager: CommandExecutionManager,
        identity: &ReActionIdentity<'_>,
        paths: &CommandExecutionPaths,
        requested_outputs: impl IntoIterator<Item = CommandExecutionOutputRef<'a>>,
        output_spec: &dyn RemoteActionResult,
        details: &RemoteCommandExecutionDetails,
        cancellations: &CancellationContext,
        phases: &mut DownloadPhases,
    ) -> ControlFlow<
        DownloadResult,
        (
            CommandExecutionManagerWithClaim,
            BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>,
        ),
    > {
        let now = Timestamp::now();
        let expires = re_expiration_from_ttl(now, output_spec.ttl(), &identity.action_key);
        // Derived from the clamped expiration rather than the raw TTL so that the two can't
        // disagree, and because the difference of two in-range datetimes can't overflow.
        let ttl = expires.duration_since(now);
        let info = CasDownloadInfo::new_execution(
            details.action_digest.dupe(),
            self.re_client.use_case,
            now,
            ttl,
        );
        let artifacts = self
            .extract_artifacts(
                artifact_fs,
                identity,
                paths,
                requested_outputs,
                output_spec,
                expires,
                &info,
                phases,
            )
            .await;

        let artifacts = match artifacts {
            Ok(artifacts) => artifacts,
            Err(e) => {
                let error: buck2_error::Error =
                    e.context(format!("action_digest={}", details.action_digest));
                let is_storage_resource_exhausted = error
                    .find_typed_context::<RemoteExecutionError>()
                    .is_some_and(|re_client_error| {
                        is_storage_resource_exhausted(re_client_error.as_ref())
                    });
                let error_type = if is_storage_resource_exhausted {
                    CommandExecutionErrorType::StorageResourceExhausted
                } else {
                    CommandExecutionErrorType::Other
                };
                return ControlFlow::Break(DownloadResult::Result(manager.error_classified(
                    "extract_artifacts",
                    error,
                    error_type,
                )));
            }
        };

        let (manager, outputs) = match self.paranoid {
            Some(paranoid) => {
                let manager = paranoid
                    .declare_cas_many(
                        self.materializer,
                        manager,
                        info,
                        artifacts.to_declare,
                        cancellations,
                    )
                    .await
                    .map_break(DownloadResult::Result)?;
                (manager, artifacts.mapped_outputs)
            }
            None => {
                // Claim the request before starting the download.
                let start = Instant::now();
                let manager = manager.claim().await;
                *phases.local_bookkeeping.get_or_insert_default() += Instant::now() - start;

                let start = Instant::now();
                let outputs = self.materialize_outputs(artifacts, info).await;
                phases.declare = Some(Instant::now() - start);

                let outputs = match outputs {
                    Ok(outputs) => outputs,
                    Err(e) => {
                        return ControlFlow::Break(DownloadResult::Result(manager.error(
                            "materialize_outputs",
                            e.context(format!("action_digest={}", details.action_digest)),
                        )));
                    }
                };

                (manager, outputs)
            }
        };

        ControlFlow::Continue((manager, outputs))
    }

    async fn extract_artifacts<'a>(
        &self,
        artifact_fs: &ArtifactFs,
        identity: &ReActionIdentity<'_>,
        paths: &CommandExecutionPaths,
        requested_outputs: impl IntoIterator<Item = CommandExecutionOutputRef<'a>>,
        output_spec: &dyn RemoteActionResult,
        expires: Timestamp,
        info: &CasDownloadInfo,
        phases: &mut DownloadPhases,
    ) -> buck2_error::Result<ExtractedArtifacts> {
        // Download process:
        // 1. merges all the outputs (files and trees) into the inputs structure
        // 2. computes the ArtifactValue for all outputs from that merged structure
        // 3. pass those new ArtifactValue to the materializer
        phases.tree_count = output_spec.output_directories().len() as u64;
        let mut start = Instant::now();
        let input_dir = paths.input_directory();
        let output_paths = paths.output_paths();
        let mut input_dir = input_dir.clone().into_builder();

        for x in output_spec.output_files() {
            let digest = FileDigest::from_re(&x.digest.digest, self.digest_config)?;
            let digest = TrackedFileDigest::new_expires(
                digest,
                expires,
                self.digest_config.cas_digest_config(),
            );

            let entry = DirectoryEntry::Leaf(ActionDirectoryMember::File(FileMetadata {
                digest,
                is_executable: x.executable,
            }));

            input_dir.insert(re_forward_path(x.name.as_str())?, entry)?;
        }

        for x in output_spec.output_symlinks() {
            let entry = DirectoryEntry::Leaf(ActionDirectoryMember::Symlink(Arc::new(
                Symlink::new(RelativePathBuf::from_system_path(Path::new(&x.target))?),
            )));
            input_dir.insert(re_forward_path(x.name.as_str())?, entry)?;
        }
        *phases.local_bookkeeping.get_or_insert_default() += Instant::now() - start;

        {
            let _permit = if let Some(semaphore) = self.output_trees_download_config.semaphore() {
                let blob_size = output_spec
                    .output_directories()
                    .iter()
                    .map(|x| x.tree_digest.size_in_bytes)
                    .sum::<i64>();

                let blob_size: u32 = blob_size
                    .try_into()
                    .unwrap_or(semaphore.max_concurrent_bytes);

                start = Instant::now();
                let permit = semaphore
                    .semaphore
                    .acquire_many(blob_size.min(semaphore.max_concurrent_bytes))
                    .await?;
                phases.tree_admission = Some(Instant::now() - start);
                Some(permit)
            } else {
                None
            };

            // Compute the re_outputs from the output_directories
            // This requires traversing the trees to find symlinks that point outside such trees
            start = Instant::now();
            let trees = self
                .re_client
                .download_typed_blobs::<RE::Tree>(
                    Some(identity),
                    output_spec
                        .output_directories()
                        .iter()
                        .map(|x| x.tree_digest.clone())
                        .collect::<Vec<_>>(),
                    info,
                )
                .boxed()
                .await
                .buck_error_context("Failed to download trees")?;
            phases.tree_fetch = Some(Instant::now() - start);

            start = Instant::now();
            for (dir, tree) in output_spec.output_directories().iter().zip(trees) {
                let entry = re_tree_to_directory(
                    &tree,
                    &expires,
                    self.digest_config,
                    self.output_trees_download_config
                        .fingerprint_re_output_trees_eagerly(),
                )?;
                input_dir.insert(
                    re_forward_path(dir.path.as_str())?,
                    DirectoryEntry::Dir(entry),
                )?;
            }
        }

        let mut to_declare = Vec::with_capacity(output_paths.len());
        let mut mapped_outputs = BuckMutInsertionMap::with_capacity(output_paths.len());

        for (requested, (path, _)) in requested_outputs.into_iter().zip(output_paths.iter()) {
            let value = extract_artifact_value(&input_dir, path, self.digest_config)?;
            if let Some(value) = value {
                to_declare.push(DeclareArtifactPayload {
                    path: requested
                        .resolve(
                            artifact_fs,
                            if requested.has_content_based_path() {
                                Some(value.content_based_path_hash())
                            } else {
                                None
                            }
                            .as_ref(),
                        )?
                        .path
                        .to_owned(),
                    artifact: value.dupe(),
                });
                mapped_outputs.insert(requested.cloned(), value);
            }
        }

        *phases.local_bookkeeping.get_or_insert_default() += Instant::now() - start;

        Ok(ExtractedArtifacts {
            to_declare,
            mapped_outputs,
        })
    }

    async fn materialize_outputs(
        &self,
        artifacts: ExtractedArtifacts,
        info: CasDownloadInfo,
    ) -> buck2_error::Result<BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>> {
        // Declare the outputs to the materializer
        self.materializer
            .declare_cas_many(Arc::new(info), artifacts.to_declare)
            .boxed()
            .await
            .buck_error_context("Failed to declare in materializer")?;

        Ok(artifacts.mapped_outputs)
    }
}

/// Takes a path that came from RE and tries to convert it to
/// a `ForwardRelativePath`. These paths are supposed to be forward relative,
/// so if the conversion fails, RE is broken.
fn re_forward_path(re_path: &str) -> buck2_error::Result<&ForwardRelativePath> {
    // RE sends us paths with trailing slash.
    ForwardRelativePath::new_trim_trailing_slashes(re_path)
        .buck_error_context("Path received from RE is not normalized.")
}

struct ExtractedArtifacts {
    to_declare: Vec<DeclareArtifactPayload>,
    mapped_outputs: BuckMutInsertionMap<CommandExecutionOutput, ArtifactValue>,
}

/// Did this download work out?
pub enum DownloadResult {
    /// Got a result: might be a success, might be a failure. Caller needs to deal with this
    /// result.
    Result(CommandExecutionResult),
}

impl FromResidual<ControlFlow<Self, Infallible>> for DownloadResult {
    fn from_residual(residual: ControlFlow<Self, Infallible>) -> Self {
        match residual {
            ControlFlow::Break(v) => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download_phases_to_proto() {
        let phases = DownloadPhases {
            tree_admission: None,
            tree_fetch: Some(Duration::from_micros(2)),
            local_bookkeeping: Some(Duration::from_micros(3)),
            declare: Some(Duration::from_micros(4)),
            tree_count: 1,
        };
        assert_eq!(
            phases.to_proto(),
            buck2_data::CacheHitTimings {
                tree_admission_us: None,
                tree_fetch_us: Some(2),
                local_bookkeeping_us: Some(3),
                declare_us: Some(4),
                tree_count: 1,
            },
            "each phase goes to its own field, and an unmeasured one stays unset"
        );
    }
}
