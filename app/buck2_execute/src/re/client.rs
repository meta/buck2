/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use anyhow::Context;
use buck2_common::cas_digest::CasDigestConfig;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::buck2_env;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_data::ReQueueAcquiringDependencies;
use buck2_data::ReQueueCancelled;
use buck2_data::ReQueueNoWorkerAvailable;
use buck2_data::ReQueueOverQuota;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::buck2_error;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::dispatch::get_dispatcher;
use buck2_events::schedule_type::SandcastleScheduleType;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_hash::BuckMutMap;
#[cfg(fbcode_build)]
use buck2_re_configuration::CASdMode;
use buck2_re_configuration::RemoteExecutionStaticMetadataImpl;
use dupe::Dupe;
use either::Either;
use futures::FutureExt;
use futures::StreamExt;
use futures::stream::BoxStream;
use itertools::Itertools;
use prost::Message;
use remote_execution as RE;
use remote_execution::ActionResultRequest;
use remote_execution::ActionResultResponse;
use remote_execution::BuckInfo;
#[cfg(fbcode_build)]
use remote_execution::ClientBuilderCommonMethods;
use remote_execution::DownloadRequest;
use remote_execution::ExecuteRequest;
use remote_execution::ExecuteWithProgressResponse;
use remote_execution::ExtendDigestsTtlRequest;
use remote_execution::GetDigestsTtlRequest;
use remote_execution::GetDigestsTtlResponse;
use remote_execution::InlinedBlobWithDigest;
use remote_execution::NamedDigest;
use remote_execution::NamedDigestWithPermissions;
use remote_execution::OperationMetadata;
use remote_execution::REClient;
use remote_execution::REClientBuilder;
use remote_execution::RemoteExecutionMetadata;
#[cfg(fbcode_build)]
use remote_execution::RemoteFetchPolicy;
use remote_execution::Stage;
use remote_execution::TActionResult2;
use remote_execution::TClientContextMetadata;
use remote_execution::TCode;
use remote_execution::TDependency;
use remote_execution::TDigest;
use remote_execution::TDuration;
use remote_execution::TExecutionPolicy;
use remote_execution::THostResourceRequirements;
use remote_execution::THostRuntimeRequirements;
use remote_execution::TLocalCacheStats;
use remote_execution::TaskState;
use remote_execution::UploadRequest;
use remote_execution::WriteActionResultRequest;
use remote_execution::WriteActionResultResponse;
use tokio::sync::Semaphore;

use crate::digest::CasDigestToReExt;
use crate::directory::ActionImmutableDirectory;
use crate::execute::action_digest::ActionDigest;
use crate::execute::blobs::ActionBlobs;
use crate::execute::executor_stage_async;
use crate::execute::kind::RemoteExecutionOrigin;
use crate::execute::manager::CommandExecutionManager;
use crate::knobs::ExecutorGlobalKnobs;
use crate::materialize::materializer::Materializer;
use crate::re::action_identity::ReActionIdentity;
use crate::re::convert::platform_to_proto;
use crate::re::digest_sampler::should_sample_action_digest;
use crate::re::error::RemoteExecutionError;
use crate::re::error::test_re_error;
use crate::re::error::test_re_error_with_group;
use crate::re::error::with_error_handler;
use crate::re::manager::RemoteExecutionConfig;
use crate::re::metadata::RemoteExecutionMetadataExt;
use crate::re::presence::CasPresence;
use crate::re::presence::NegativeCache;
use crate::re::presence::Presence;
use crate::re::presence::TtlBackend;
use crate::re::queue_stats::QueueStats;
use crate::re::remote_action_result::ExecuteResponseWithQueueStats;
use crate::re::remote_action_result::RemoteActionResult;
use crate::re::stats::LocalCacheRemoteExecutionClientStats;
use crate::re::stats::LocalCacheStats;
use crate::re::stats::OpStats;
use crate::re::stats::RemoteExecutionClientOpStats;
use crate::re::stats::RemoteExecutionClientStats;
use crate::re::ttl::re_expiration_from_ttl;
use crate::re::uploader::UploadStats;
use crate::re::uploader::Uploader;

pub enum ActionCacheWriteType {
    LocalCacheUpload,
    PermissionCheck,
    RemoteDepFile,
    WriteAction,
}
impl ActionCacheWriteType {
    fn as_str(&self) -> &'static str {
        match *self {
            ActionCacheWriteType::LocalCacheUpload => "local_cache_upload",
            ActionCacheWriteType::PermissionCheck => "permission_check",
            ActionCacheWriteType::RemoteDepFile => "remote_dep_file",
            ActionCacheWriteType::WriteAction => "write_action",
        }
    }
}

pub enum CancellationReason {
    NotSpecified,
    ReQueueTimeout,
}

#[derive(Default)]
pub struct Cancelled {
    pub reason: Option<CancellationReason>,
}

#[derive(Clone, Dupe, Allocative)]
pub struct RemoteExecutionClient {
    data: Arc<RemoteExecutionClientData>,
}

pub enum ExecutionStarted {
    Yes,
    No,
}

// The large one is the actual default case
#[allow(clippy::large_enum_variant)]
pub enum ExecuteResponseOrCancelled {
    Response(ExecuteResponseWithQueueStats),
    Cancelled(Cancelled, QueueStats, ExecutionStarted),
}

#[derive(Allocative)]
struct RemoteExecutionClientData {
    client: RemoteExecutionClientImpl,
    uploads: OpStats,
    downloads: OpStats,
    action_cache: OpStats,
    executes: OpStats,
    materializes: OpStats,
    write_action_results: OpStats,
    get_digests_ttl: OpStats,
    extend_digest_ttl: OpStats,
    local_cache: LocalCacheStats,
    persistent_cache_mode: Option<String>,
    presence_check_concurrency: usize,
    presence_check_max_batch: usize,
}

/// Issues presence-check RPCs through a client, under whatever use case each batch belongs to.
struct ClientTtlBackend {
    client: RemoteExecutionClient,
}

#[async_trait::async_trait]
impl TtlBackend for ClientTtlBackend {
    async fn get_digests_ttl(
        &self,
        digests: Vec<TDigest>,
        use_case: RemoteExecutorUseCase,
        negative_cache: NegativeCache,
    ) -> buck2_error::Result<GetDigestsTtlResponse> {
        self.client
            .get_digests_ttl(
                digests,
                &use_case.metadata(None),
                negative_cache.is_for_upload(),
            )
            .await
    }
}

#[cfg(fbcode_build)]
fn map_casd_mode_into_remote_fetch_policy(casd_mode: &CASdMode) -> RemoteFetchPolicy {
    match casd_mode {
        CASdMode::LocalWithoutSync => RemoteFetchPolicy::LOCAL_FETCH_WITHOUT_SYNC,
        CASdMode::Remote => RemoteFetchPolicy::REMOTE_FETCH,
        CASdMode::LocalWithSync => RemoteFetchPolicy::LOCAL_FETCH_WITH_SYNC,
    }
}

impl RemoteExecutionClient {
    pub async fn new(re_config: &RemoteExecutionConfig) -> buck2_error::Result<Self> {
        if buck2_env!("BUCK2_TEST_FAIL_CONNECT", bool, applicability = testing)? {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Injected RE Connection error"
            ));
        }

        // Creating the client normally takes seconds. Every command sharing the connection waits
        // on it, so an attempt that wedges must fail rather than hang them all. 0 removes the bound.
        let timeout_s = buck2_env!("BUCK2_RE_CONNECT_TIMEOUT_S", type = u64, default = 120)?;
        let create = RemoteExecutionClientImpl::new(re_config);
        let client = if timeout_s == 0 {
            create.await?
        } else {
            tokio::time::timeout(Duration::from_secs(timeout_s), create)
                .await
                .map_err(|_| {
                    buck2_error!(
                        buck2_error::ErrorTag::ReDeadlineExceeded,
                        "Creating the RE client did not finish within {}s. If this persists, run `buck2 kill`",
                        timeout_s
                    )
                })??
        };

        let persistent_cache_mode = client.persistent_cache_mode.clone();
        Ok(Self {
            data: Arc::new(RemoteExecutionClientData {
                client,
                uploads: OpStats::default(),
                downloads: OpStats::default(),
                action_cache: OpStats::default(),
                executes: OpStats::default(),
                materializes: OpStats::default(),
                write_action_results: OpStats::default(),
                get_digests_ttl: OpStats::default(),
                extend_digest_ttl: OpStats::default(),
                local_cache: Default::default(),
                persistent_cache_mode,
                presence_check_concurrency: re_config.static_metadata.presence_check_concurrency(),
                presence_check_max_batch: re_config.static_metadata.presence_check_max_batch(),
            }),
        })
    }

    pub async fn new_retry(re_config: &RemoteExecutionConfig) -> buck2_error::Result<Self> {
        // Loop happens times-1 times at most
        for i in 1..re_config.connection_retries {
            match Self::new(re_config).await {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if e.find_typed_context::<RemoteExecutionError>().is_none() {
                        // If we cannot connect to RE due to some non-RE error, we should not retry
                        // And should just return the error immediately as it's unlikely to be flakey
                        return Err(e);
                    }

                    tracing::debug!(
                        "Failed to connect to RE, retrying after sleeping {} seconds: {:#?}",
                        i,
                        e
                    );
                    tokio::time::sleep(Duration::from_secs(i as u64)).await;
                }
            }
        }
        Self::new(re_config).await
    }

    pub async fn action_cache(
        &self,
        action_digest: ActionDigest,
        metadata: &RemoteExecutionMetadata,
        use_case: RemoteExecutorUseCase,
        platform: &RE::Platform,
    ) -> buck2_error::Result<Option<ActionResultResponse>> {
        let _ac = self.data.client.action_cache_semaphore.acquire().await;

        self.data
            .action_cache
            .op(self
                .data
                .client
                .action_cache(action_digest, metadata, use_case, platform))
            .await
    }

    pub async fn upload(
        &self,
        fs: &ProjectRoot,
        materializer: &dyn Materializer,
        blobs: &ActionBlobs,
        dir_path: &ProjectRelativePath,
        input_dir: &ActionImmutableDirectory,
        use_case: RemoteExecutorUseCase,
        identity: Option<&ReActionIdentity<'_>>,
    ) -> buck2_error::Result<UploadStats> {
        // Actually upload to CAS
        let _cas = self.data.client.cas_semaphore.acquire().await;

        self.data
            .uploads
            .op(Uploader::upload(
                fs,
                self,
                materializer,
                dir_path,
                input_dir,
                blobs,
                use_case,
                identity,
            ))
            .await
    }

    /// Uploads whichever of `blobs` the CAS does not hold, as `upload` does for an action's
    /// inputs: behind the CAS semaphore, with the presence check in front.
    pub async fn upload_blobs(
        &self,
        blobs: &ActionBlobs,
        use_case: RemoteExecutorUseCase,
    ) -> buck2_error::Result<UploadStats> {
        let _cas = self.data.client.cas_semaphore.acquire().await;

        self.data
            .uploads
            .op(Uploader::upload_blobs(self, blobs, use_case))
            .await
    }

    pub async fn upload_files_and_directories(
        &self,
        files_with_digest: Vec<NamedDigest>,
        directories: Vec<remote_execution::Path>,
        inlined_blobs_with_digest: Vec<InlinedBlobWithDigest>,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<()> {
        self.data
            .uploads
            .op(self.data.client.upload_files_and_directories(
                files_with_digest,
                directories,
                inlined_blobs_with_digest,
                metadata,
            ))
            .await
    }

    pub async fn execute<'a>(
        &self,
        action_digest: ActionDigest,
        platform: &RE::Platform,
        dependencies: impl IntoIterator<Item = &'a RemoteExecutorDependency>,
        re_gang_workers: &[buck2_core::execution_types::executor_config::ReGangWorker],
        use_case: RemoteExecutorUseCase,
        identity: &ReActionIdentity<'_>,
        manager: &mut CommandExecutionManager,
        skip_cache_read: bool,
        skip_cache_write: bool,
        re_max_queue_time: Option<Duration>,
        re_resource_units: Option<i64>,
        knobs: &ExecutorGlobalKnobs,
        meta_internal_extra_params: &MetaInternalExtraParams,
        worker_tool_action_digest: Option<ActionDigest>,
        priority: Option<i32>,
    ) -> buck2_error::Result<ExecuteResponseOrCancelled> {
        self.data
            .executes
            .op(self.data.client.execute(
                action_digest,
                platform,
                dependencies,
                re_gang_workers,
                use_case,
                identity,
                manager,
                skip_cache_read,
                skip_cache_write,
                re_max_queue_time,
                re_resource_units,
                knobs,
                meta_internal_extra_params,
                worker_tool_action_digest,
                priority,
            ))
            .await
    }

    pub async fn materialize_files(
        &self,
        files: Vec<NamedDigestWithPermissions>,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<()> {
        let stat = self
            .data
            .materializes
            .op(self.data.client.materialize_files(files, metadata))
            .await?;
        self.data.local_cache.update(&stat);
        Ok(())
    }

    pub async fn download_typed_blobs<T: Message + Default>(
        &self,
        identity: Option<&ReActionIdentity<'_>>,
        digests: Vec<TDigest>,
        use_case: RemoteExecutorUseCase,
    ) -> buck2_error::Result<Vec<T>> {
        self.data
            .downloads
            .op(self
                .data
                .client
                .download_typed_blobs(identity, digests, use_case))
            .await
            .map(|r| {
                self.data.local_cache.update(&r.1);
                r.0
            })
    }

    pub async fn download_blob(
        &self,
        digest: &TDigest,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<Vec<u8>> {
        self.data
            .downloads
            .op(self.data.client.download_blob(digest, metadata))
            .await
            .map(|r| {
                self.data.local_cache.update(&r.1);
                r.0
            })
    }

    pub async fn upload_blob(
        &self,
        blob: InlinedBlobWithDigest,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<TDigest> {
        self.data
            .uploads
            .op(self.data.client.upload_blob(blob, metadata))
            .await
    }

    pub async fn get_digests_ttl(
        &self,
        digests: Vec<TDigest>,
        metadata: &RemoteExecutionMetadata,
        is_for_upload: bool,
    ) -> buck2_error::Result<GetDigestsTtlResponse> {
        self.data
            .get_digests_ttl
            .op(self
                .data
                .client
                .get_digests_ttl(digests, metadata, is_for_upload))
            .await
    }

    /// Whether the CAS has each of `digests`, and until when; see [`CasPresence::check`]. The
    /// RPCs are attributed to `use_case`.
    pub async fn check_presence(
        &self,
        use_case: RemoteExecutorUseCase,
        digests: Vec<TrackedFileDigest>,
        negative_cache: NegativeCache,
    ) -> buck2_error::Result<Vec<Presence>> {
        let backend: Arc<dyn TtlBackend> = Arc::new(ClientTtlBackend {
            client: self.dupe(),
        });
        CasPresence::global(
            self.data.presence_check_concurrency,
            self.data.presence_check_max_batch,
        )
        .check(backend, use_case, negative_cache, digests)
        .await
    }

    pub async fn get_digest_expirations(
        &self,
        digests: Vec<TDigest>,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<Vec<(TDigest, jiff::Timestamp)>> {
        let now = jiff::Timestamp::now();
        let ttls = self.get_digests_ttl(digests, metadata, false).await?;
        Ok(ttls
            .digests_with_ttl
            .into_iter()
            .map(|t| {
                let expiration = re_expiration_from_ttl(now, t.ttl, &t.digest);
                (t.digest, expiration)
            })
            .collect())
    }

    pub async fn extend_digest_ttl(
        &self,
        digests: Vec<TDigest>,
        ttl: Duration,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<()> {
        self.data
            .extend_digest_ttl
            .op(self.data.client.extend_digest_ttl(digests, ttl, metadata))
            .await
    }

    pub async fn write_action_result(
        &self,
        digest: ActionDigest,
        result: &mut TActionResult2,
        use_case: RemoteExecutorUseCase,
        platform: &RE::Platform,
        write_type: ActionCacheWriteType,
    ) -> buck2_error::Result<WriteActionResultResponse> {
        let _ac = self.data.client.action_cache_semaphore.acquire().await;

        self.data
            .write_action_results
            .op(self
                .data
                .client
                .write_action_result(digest, result, use_case, platform, write_type))
            .await
    }

    pub fn get_session_id(&self) -> &str {
        self.data.client.get_session_id()
    }

    pub fn get_persistent_cache_mode(&self) -> Option<String> {
        self.data.persistent_cache_mode.clone()
    }

    pub fn get_experiment_name(&self) -> buck2_error::Result<Option<String>> {
        self.data
            .client
            .client()
            .get_experiment_name()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::ReExperimentName))
    }

    pub fn release_temporary_memory(&self) -> buck2_error::Result<()> {
        self.data
            .client
            .client()
            .release_temporary_memory()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
    }

    pub fn fill_network_stats(&self, stats: &mut RemoteExecutionClientStats) {
        stats.uploads = RemoteExecutionClientOpStats::from(&self.data.uploads);
        stats.downloads = RemoteExecutionClientOpStats::from(&self.data.downloads);
        stats.executes = RemoteExecutionClientOpStats::from(&self.data.executes);
        stats.action_cache = RemoteExecutionClientOpStats::from(&self.data.action_cache);
        stats.write_action_results =
            RemoteExecutionClientOpStats::from(&self.data.write_action_results);
        stats.materializes = RemoteExecutionClientOpStats::from(&self.data.materializes);
        stats.get_digest_expirations =
            RemoteExecutionClientOpStats::from(&self.data.get_digests_ttl);
        stats.local_cache = LocalCacheRemoteExecutionClientStats::from(&self.data.local_cache);
    }

    pub(super) fn get_raw_re_client(&self) -> &REClient {
        self.data.client.client()
    }
}

#[derive(Allocative)]
struct RemoteExecutionClientImpl {
    #[allocative(skip)]
    client: Option<REClient>,
    skip_remote_cache: bool,
    /// How many simultaneous requests to RE
    #[allocative(skip)]
    cas_semaphore: Arc<Semaphore>,
    /// Bounds concurrent action cache reads/writes. Particularly at build start thousands
    /// of actions become ready at once, and this prevents us from issuing requests
    /// faster than they can be filled, reducing the transient memory we have to hold.
    #[allocative(skip)]
    action_cache_semaphore: Arc<Semaphore>,
    /// How many simultaneous execute requests to RE
    #[allocative(skip)]
    exec_semaphore: Arc<Semaphore>,
    /// How many files we can be downloading concurrently.
    #[allocative(skip)]
    download_files_semapore: Arc<Semaphore>,
    /// How many files to kick off downloading concurrently for one request. This should be smaller
    /// than the files semaphore to ensure we can actually *acquire* that semaphore.
    download_chunk_size: usize,
    /// Preserve file symlinks as symlinks when uploading action result.
    respect_file_symlinks: bool,
    persistent_cache_mode: Option<String>,
}

fn re_platform(x: &RE::Platform) -> remote_execution::TPlatform {
    #[allow(clippy::needless_update)] // Defaults are needed internally but not in OSS
    remote_execution::TPlatform {
        properties: x
            .properties
            .iter()
            .map(|x| remote_execution::TProperty {
                name: x.name.clone(),
                value: x.value.clone(),
                ..Default::default()
            })
            .collect::<Vec<_>>(),
        ..Default::default()
    }
}

fn anticipated_queue_duration(
    event: &ExecuteWithProgressResponse,
) -> anyhow::Result<Option<Duration>> {
    // Return a queue estimate even if RE dequeues immediately
    if let Some(duration) = buck2_env!(
        "BUCK2_TEST_RE_QUEUE_ESTIMATE_S",
        type=u64,
        applicability = testing
    )
    // Stringify the error because we can't deal with buck2_errors here
    .map_err(|e| anyhow::anyhow!(e))?
    {
        return Ok(Some(Duration::from_secs(duration)));
    }

    if let Some(info) = &event.metadata.task_info {
        // TODO make RE report same value as estimated_queue_time_ms, switch to that then stop reporting new_estimated_queue_time_ms
        let estimated_queue_time_ms = info.new_estimated_queue_time_ms;

        let est = u64::try_from(estimated_queue_time_ms)
            .context("estimated_queue_time_ms from RE is negative")?;
        return Ok(Some(Duration::from_millis(est)));
    }
    Ok(None)
}

fn action_result_costs(result: &remote_execution::TActionResult2) -> (u64, u64) {
    let storage: u64 = result
        .output_files
        .iter()
        .map(|f| f.digest.digest.size_in_bytes.max(0) as u64)
        .chain(
            result
                .output_directories
                .iter()
                // Note: This size is not at all the size of the constituent files;
                // however we can't know that value without talking to CAS, so this
                // will have to do.
                .map(|d| d.tree_digest.size_in_bytes.max(0) as u64),
        )
        .sum();
    let meta = &result.execution_metadata;
    let execution_time = meta
        .execution_completed_timestamp
        .saturating_duration_since(&meta.execution_start_timestamp);
    let compute = execution_time.as_millis() as u64;
    (storage, compute)
}

fn trace_action_digest(
    digest: &ActionDigest,
    ttl_secs: Option<i64>,
    use_case: RemoteExecutorUseCase,
    event: buck2_data::action_digest_trace::ActionDigestTraceEvent,
    event_subtype: Option<&'static str>,
    storage_cost_bytes: Option<u64>,
    compute_cost_ms: Option<u64>,
) {
    if let Some(weight) = should_sample_action_digest(digest) {
        let dispatcher = get_dispatcher();
        let metadata = buck2_events::metadata::collect(dispatcher.daemon_id());
        let schedule_type = SandcastleScheduleType::new()
            .ok()
            .and_then(|s| s.as_str().map(|s| s.to_owned()));
        dispatcher.instant_event(buck2_data::ActionDigestTrace {
            metadata,
            action_digest: digest.to_string(),
            event: event as i32,
            event_subtype: event_subtype.map(|s| s.to_owned()),
            ttl_secs: ttl_secs.filter(|t| *t >= 0).map(|t| t as u64),
            re_use_case: use_case.as_str().to_owned(),
            weight,
            schedule_type,
            storage_cost_bytes,
            compute_cost_ms,
        });
    }
}

// Debugging tool: A set of action digests to pretend that we got cache misses on regardless of the
// actual state of the remote cache
//
// After we execute an action once, we no longer want to pretend that we got cache misses on it if
// we execute it again (say, on a subsequent build); the `AtomicBool` in the value deals with that,
// it's true after the first time we execute the action
static INDUCED_CACHE_MISSES: OnceLock<Option<BuckMutMap<ActionDigest, AtomicBool>>> =
    OnceLock::new();

/// Loads `BUCK2_INDUCED_CACHE_MISSES`, if set. The daemon calls this once at startup, with the
/// digest config its action digests use; until then nothing is an induced cache miss.
pub fn init_induced_cache_misses(config: CasDigestConfig) -> buck2_error::Result<()> {
    let misses = match std::env::var("BUCK2_INDUCED_CACHE_MISSES") {
        Err(_) => None,
        Ok(path) => {
            let path = AbsNormPath::new(&path)?;
            let contents = fs_util::read_to_string(path).categorize_input()?;
            Some(
                contents
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(|line| {
                        ActionDigest::parse_digest(line, config)
                            .map(|(digest, _)| (digest, AtomicBool::new(false)))
                            .with_buck_error_context(|| {
                                format!("Invalid action digest `{line}` in `{path}`")
                            })
                    })
                    .collect::<buck2_error::Result<_>>()?,
            )
        }
    };
    INDUCED_CACHE_MISSES.get_or_init(|| misses);
    Ok(())
}

fn induced_cache_misses() -> Option<&'static BuckMutMap<ActionDigest, AtomicBool>> {
    INDUCED_CACHE_MISSES
        .get()
        .and_then(|misses| misses.as_ref())
}

/// Whether `BUCK2_INDUCED_CACHE_MISSES` is set at all, so that `is_induced_cache_miss` can say yes.
pub fn induced_cache_misses_configured() -> bool {
    induced_cache_misses().is_some()
}

/// Whether `BUCK2_INDUCED_CACHE_MISSES` lists this action and it has not been executed remotely
/// yet.
pub fn is_induced_cache_miss(action_digest: &ActionDigest) -> bool {
    induced_cache_misses().is_some_and(|misses| {
        misses
            .get(action_digest)
            .is_some_and(|executed| !executed.load(std::sync::atomic::Ordering::Relaxed))
    })
}

impl RemoteExecutionClientImpl {
    async fn new(re_config: &RemoteExecutionConfig) -> buck2_error::Result<Self> {
        let op_name = "REClientBuilder";
        tracing::info!("Creating a new RE client");

        let res: buck2_error::Result<Self> = try {
            let download_concurrency =
                buck2_env!("BUCK2_RE_DOWNLOAD_CONCURRENCY", type=usize, default=256)?;

            // Split things up into smaller chunks.
            let download_chunk_size = std::cmp::max(download_concurrency / 8, 1);
            let static_metadata = &re_config.static_metadata;

            #[allow(unused_mut)]
            let mut persistent_cache_mode = None;
            #[cfg(fbcode_build)]
            let client = {
                use buck2_fs::fs_util;
                use remote_execution::CASDaemonClientCfg;
                use remote_execution::CopyPolicy;
                use remote_execution::CurlReactorConfig;
                use remote_execution::EmbeddedCASDaemonClientCfg;
                use remote_execution::RichClientMode;
                use remote_execution::TTLExtendingConfig;
                use remote_execution::ThreadConfig;
                use remote_execution::create_default_config;

                let mut re_client_config = create_default_config();

                let mut embedded_cas_daemon_config = EmbeddedCASDaemonClientCfg {
                    connection_count: static_metadata.cas_connection_count,
                    address: static_metadata.cas_address.clone(),
                    name: "buck2".to_owned(),
                    ..Default::default()
                };

                if re_config.is_paranoid_mode {
                    // Dedupe is not compatible with us downloading blobs and moving them.
                    embedded_cas_daemon_config.disable_download_dedup = true;
                }

                // buck2 makes find_missing calls for the same blobs
                // so having a 50Mb cache to amortize that
                embedded_cas_daemon_config
                    .cache_config
                    .find_missing_cache_size_byte = 50 << 20;
                // a small cache maps inodes to digests
                // useful for both uploads and downloads
                embedded_cas_daemon_config.cache_config.digest_cache_size = 100000;

                // If a shared CAS cache directory is configured, we
                // want to tell the RE client to rely on an external
                // CAS daemon to manage the cache.
                if let Some(external_casd_address) = &static_metadata.shared_casd_address {
                    use buck2_re_configuration::CASdAddress;
                    use buck2_re_configuration::CopyPolicy as Buck2CopyPolicy;
                    use remote_execution::RemoteCASdAddress;
                    use remote_execution::RemoteCacheConfig;
                    use remote_execution::RemoteCacheSyncConfig;

                    let policies: buck2_error::Result<(RemoteFetchPolicy, RemoteFetchPolicy)> =
                        if let Some(legacy_mode) = &static_metadata.legacy_shared_casd_mode {
                            let upper_legacy_mode = legacy_mode.to_uppercase();
                            match upper_legacy_mode.trim() {
                                "BIG_FILES" => Ok((
                                    RemoteFetchPolicy::LOCAL_FETCH_WITHOUT_SYNC,
                                    RemoteFetchPolicy::REMOTE_FETCH,
                                )),
                                "ALL_FILES" => Ok((
                                    RemoteFetchPolicy::REMOTE_FETCH,
                                    RemoteFetchPolicy::REMOTE_FETCH,
                                )),
                                "ALL_FILES_LOCAL_WITHOUT_SYNC" => Ok((
                                    RemoteFetchPolicy::LOCAL_FETCH_WITHOUT_SYNC,
                                    RemoteFetchPolicy::LOCAL_FETCH_WITHOUT_SYNC,
                                )),
                                unknown => {
                                    return Err(buck2_error!(
                                        buck2_error::ErrorTag::Input,
                                        "Unknown RemoteCacheManagerMode: {}",
                                        unknown
                                    ));
                                }
                            }
                        } else {
                            Ok((
                                static_metadata
                                    .shared_casd_mode_small_files
                                    .as_ref()
                                    .map(map_casd_mode_into_remote_fetch_policy)
                                    .unwrap_or(RemoteFetchPolicy::LOCAL_FETCH_WITHOUT_SYNC),
                                static_metadata
                                    .shared_casd_mode_large_files
                                    .as_ref()
                                    .map(map_casd_mode_into_remote_fetch_policy)
                                    .unwrap_or(RemoteFetchPolicy::LOCAL_FETCH_WITHOUT_SYNC),
                            ))
                        };
                    let (small_files_policy, large_files_policy) = policies?;

                    let remote_cache_config = {
                        let mut remote_cache_config = RemoteCacheConfig {
                            small_files: small_files_policy,
                            large_files: large_files_policy,
                            address: match external_casd_address {
                                CASdAddress::Uds(path) => RemoteCASdAddress::uds_path(path.clone()),
                                CASdAddress::Tcp(port) => RemoteCASdAddress::tcp_port(*port as i32),
                            },
                            sync_files_config: Some(RemoteCacheSyncConfig {
                                max_batch_size: static_metadata
                                    .shared_casd_cache_sync_max_batch_size
                                    .unwrap_or(100)
                                    as i32,
                                max_delay_ms: static_metadata
                                    .shared_casd_cache_sync_max_delay_ms
                                    .unwrap_or(1000)
                                    as i32,
                                wal_buckets: static_metadata
                                    .shared_casd_cache_sync_wal_files_count
                                    .unwrap_or(8)
                                    as i32,
                                wal_max_file_size: static_metadata
                                    .shared_casd_cache_sync_wal_file_max_size
                                    .unwrap_or(200 << 20)
                                    as i64,
                                ..Default::default()
                            }),
                            destination_path_hint: Some(
                                re_config
                                    .buck_out_path
                                    .to_str()
                                    .buck_error_context("invalid destination path")?
                                    .to_owned(),
                            ),
                            sync_copy_policy: match static_metadata
                                .shared_casd_copy_policy
                                .clone()
                                .unwrap_or(Buck2CopyPolicy::Copy)
                            {
                                Buck2CopyPolicy::Copy => CopyPolicy::FULL_COPY,
                                Buck2CopyPolicy::Reflink => CopyPolicy::SOFT_COPY,
                                Buck2CopyPolicy::Hybrid => CopyPolicy::HYBRID_COPY,
                            },
                            ..Default::default()
                        };
                        if let Some(tls) = static_metadata.shared_casd_use_tls {
                            remote_cache_config.use_tls = tls;
                        }
                        remote_cache_config
                    };
                    persistent_cache_mode = Some(format!(
                        "small=>{small_files_policy:?}, large=>{large_files_policy:?}"
                    ));

                    embedded_cas_daemon_config.remote_cache_config = Some(remote_cache_config);
                    embedded_cas_daemon_config.cache_config.writable_cache = false;
                    embedded_cas_daemon_config
                        .cache_config
                        .downloads_cache_config
                        .dir_path = static_metadata.shared_casd_cache_path.clone();
                }

                // prevents downloading the same trees (dirs)
                embedded_cas_daemon_config
                    .rich_client_config
                    .get_tree_cache_size = 50 << 20; // 50 Mb

                // disabling zippy rich client until we have limits in place
                embedded_cas_daemon_config
                    .rich_client_config
                    .zdb_client_mode = if static_metadata.use_zippy_rich_client {
                    RichClientMode::ENABLED
                } else {
                    RichClientMode::DISABLED
                };
                embedded_cas_daemon_config.rich_client_config.disable_p2p =
                    !static_metadata.use_p2p;
                embedded_cas_daemon_config
                    .rich_client_config
                    .enable_rich_client = static_metadata.use_manifold_rich_client;

                if let Some(channels) = static_metadata.rich_client_channels_per_blob {
                    embedded_cas_daemon_config
                        .rich_client_config
                        .number_of_parallel_channels = channels;
                }

                if let Some(attempt_timeout) = static_metadata.rich_client_attempt_timeout_ms {
                    embedded_cas_daemon_config
                        .rich_client_config
                        .attempt_timeout_ms = attempt_timeout;
                }

                embedded_cas_daemon_config.force_enable_deduplicate_find_missing =
                    static_metadata.force_enable_deduplicate_find_missing;

                // Will either choose the SOFT_COPY (on some linux fs like btrfs/extfs etc, on Mac if using APFS) or FULL_COPY otherwise
                embedded_cas_daemon_config.copy_policy = CopyPolicy::BEST_AVAILABLE;
                embedded_cas_daemon_config.materialization_mount_path = Some(
                    re_config
                        .buck_out_path
                        .to_str()
                        .buck_error_context("invalid meterialization_mount_path")?
                        .to_owned(),
                );

                if static_metadata.cas_thread_count_ratio == 0.0 {
                    embedded_cas_daemon_config.thread_count =
                        ThreadConfig::fixed_thread_count(static_metadata.cas_thread_count);
                } else {
                    embedded_cas_daemon_config.thread_count =
                        ThreadConfig::thread_count_ratio(static_metadata.cas_thread_count_ratio);
                }

                // make sure that outputs are writable
                // otherwise actions that are modifying outputs will fail due to a permission error
                embedded_cas_daemon_config.writable_outputs = true;

                embedded_cas_daemon_config.client_label = static_metadata
                    .cas_client_label
                    .clone()
                    .unwrap_or_else(|| "".to_owned());

                let minimal_blob_ttl_threshold =
                    static_metadata.minimal_blob_ttl_seconds.unwrap_or(3600);
                let remaining_ttl_fraction_refresh_threshold = static_metadata
                    .remaining_ttl_fraction_refresh_threshold
                    .unwrap_or(0.1)
                    as f64;
                let remaining_ttl_random_extra_threshold = static_metadata
                    .remaining_ttl_random_extra_threshold
                    .unwrap_or(0.25)
                    as f64;
                embedded_cas_daemon_config.ttl_extending_config = Some(TTLExtendingConfig {
                    blocking_ttl_extending_seconds_threshold: minimal_blob_ttl_threshold,
                    remaining_ttl_fraction_refresh_threshold,
                    remaining_ttl_random_extra_threshold,
                    ..Default::default()
                });
                embedded_cas_daemon_config.action_cache_ttl_extending_config =
                    Some(TTLExtendingConfig {
                        blocking_ttl_extending_seconds_threshold: minimal_blob_ttl_threshold,
                        remaining_ttl_fraction_refresh_threshold,
                        remaining_ttl_random_extra_threshold,
                        ..Default::default()
                    });

                re_client_config.cas_client_config =
                    CASDaemonClientCfg::embedded_config(embedded_cas_daemon_config);
                if let Some(logs_dir_path) = &re_config.logs_dir_path {
                    // make sure that the log dir exists as glog is expecting that :(
                    fs_util::create_dir_all(logs_dir_path)?;
                    re_client_config.log_file_location = Some(
                        logs_dir_path
                            .to_str()
                            .buck_error_context("Invalid log_file_location")?
                            .to_owned(),
                    );
                    // keep last 10 sessions (similar to a number of buck builds)
                    re_client_config.log_rollup_window_size = 10;
                }

                if static_metadata.curl_reactor_max_number_of_retries.is_some()
                    || static_metadata.curl_reactor_connection_timeout_ms.is_some()
                    || static_metadata.curl_reactor_request_timeout_ms.is_some()
                {
                    let mut curl_config = CurlReactorConfig {
                        ..Default::default()
                    };

                    if let Some(max_number_of_retries) =
                        static_metadata.curl_reactor_max_number_of_retries
                    {
                        curl_config.max_number_of_retries = max_number_of_retries;
                    }

                    if let Some(request_timeout_ms) =
                        static_metadata.curl_reactor_request_timeout_ms
                    {
                        curl_config.request_timeout_ms = request_timeout_ms;
                    }

                    if let Some(connection_timeout_ms) =
                        static_metadata.curl_reactor_connection_timeout_ms
                    {
                        curl_config.connection_timeout_ms = connection_timeout_ms;
                    }

                    re_client_config.curl_reactor_config = Some(curl_config);
                }

                re_client_config.features_config_path = static_metadata
                    .features_config_path
                    .as_deref()
                    .unwrap_or(
                        if static_metadata.use_zippy_rich_client && cfg!(target_os = "linux") {
                            if static_metadata.shared_casd_address.is_some() {
                                "remote_execution/features/client_buck2_pc"
                            } else {
                                "remote_execution/features/client_buck2"
                            }
                        } else {
                            "remote_execution/features/client_buck2_alternative"
                        },
                    )
                    .to_owned();

                re_client_config.client_config_path = static_metadata
                    .client_config_path
                    .as_deref()
                    .unwrap_or("remote_execution/client/configs/buck")
                    .to_owned();

                re_client_config.disable_fallocate = static_metadata.disable_fallocate;

                re_client_config
                    .thrift_execution_client_config
                    .concurrency_limit = static_metadata.execution_concurrency_limit;

                if let Some(engine_tier) = static_metadata.engine_tier.to_owned() {
                    re_client_config.thrift_execution_client_config.tier = engine_tier;
                }

                if static_metadata.engine_host.is_some() || static_metadata.engine_port.is_some() {
                    if static_metadata.engine_host.is_some()
                        && let Some(engine_port) = static_metadata.engine_port
                    {
                        re_client_config.thrift_execution_client_config.host_port =
                            Some(remote_execution::HostPort {
                                host: static_metadata.engine_host.clone().unwrap(),
                                port: engine_port,
                                ..Default::default()
                            });
                    } else {
                        return Err(buck2_error!(
                            buck2_error::ErrorTag::Input,
                            "Both engine_host and engine_port must be set if either is set"
                        ));
                    }
                }

                // TODO(ndmitchell): For now, we just drop RE log messages, but ideally we'd put them in our log stream.
                let logger = slog::Logger::root(slog::Discard, slog::o!());
                // TODO T179215751: If RE client fails we don't get the RE session ID and we can't find the RE logs.
                // Better to generate the RE session ID ourselves and pass it to the RE client.
                with_error_handler(
                    op_name,
                    "<none>",
                    REClientBuilder::new(re_config.fb)
                        .with_config(re_client_config)
                        .with_cancellation(static_metadata.enable_download_cancellation)
                        .with_logger(logger)
                        .build_and_connect()
                        .await,
                )?
            };

            #[cfg(not(fbcode_build))]
            let client = {
                with_error_handler(
                    op_name,
                    "<none>",
                    REClientBuilder::build_and_connect(&static_metadata.0).await,
                )?
            };

            let respect_file_symlinks = {
                #[cfg(fbcode_build)]
                {
                    static_metadata.respect_file_symlinks
                }
                #[cfg(not(fbcode_build))]
                {
                    false
                }
            };

            Self {
                client: Some(client),
                skip_remote_cache: re_config.skip_remote_cache,
                cas_semaphore: Arc::new(Semaphore::new(static_metadata.cas_semaphore_size())),
                action_cache_semaphore: Arc::new(Semaphore::new(
                    static_metadata.action_cache_semaphore_size(),
                )),
                exec_semaphore: Arc::new(Semaphore::new(static_metadata.exec_semaphore_size())),
                download_files_semapore: Arc::new(Semaphore::new(download_concurrency)),
                download_chunk_size,
                respect_file_symlinks,
                persistent_cache_mode,
            }
        };

        res
    }

    fn client(&self) -> &REClient {
        self.client
            .as_ref()
            .expect("REClient is always present unless dropped")
    }

    fn get_session_id(&self) -> &str {
        self.client().get_session_id()
    }

    async fn action_cache(
        &self,
        action_digest: ActionDigest,
        metadata: &RemoteExecutionMetadata,
        use_case: RemoteExecutorUseCase,
        platform: &RE::Platform,
    ) -> buck2_error::Result<Option<ActionResultResponse>> {
        if is_induced_cache_miss(&action_digest) {
            return Ok(None);
        }

        let lookup = self.client().get_action_cache_client().get_action_result(
            metadata,
            ActionResultRequest {
                digest: action_digest.to_re(),
                platform: Some(re_platform(platform)),
                ..Default::default()
            },
        );
        let res = with_error_handler("action_cache", self.get_session_id(), lookup.await);

        let res = match res {
            Ok(r) => Some(r),
            Err(e) => match e.find_typed_context::<RemoteExecutionError>() {
                Some(re_err) if re_err.code == TCode::NOT_FOUND => None,
                _ => return Err(e),
            },
        };
        trace_action_digest(
            &action_digest,
            res.as_ref().map(|r| r.ttl),
            use_case,
            if res.is_some() {
                buck2_data::action_digest_trace::ActionDigestTraceEvent::CacheHit
            } else {
                buck2_data::action_digest_trace::ActionDigestTraceEvent::CacheMiss
            },
            None,
            None,
            None,
        );
        Ok(res)
    }

    async fn upload_files_and_directories(
        &self,
        files_with_digest: Vec<NamedDigest>,
        directories: Vec<remote_execution::Path>,
        inlined_blobs_with_digest: Vec<InlinedBlobWithDigest>,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<()> {
        let upload = self.client().get_cas_client().upload(
            metadata,
            UploadRequest {
                files_with_digest: Some(files_with_digest),
                inlined_blobs_with_digest: Some(inlined_blobs_with_digest),
                directories: Some(directories),
                upload_only_missing: true,
                ..Default::default()
            },
        );
        with_error_handler(
            "upload_files_and_directories",
            self.get_session_id(),
            upload.await,
        )?;
        Ok(())
    }

    async fn execute_impl(
        &self,
        metadata: &RemoteExecutionMetadata,
        request: ExecuteRequest,
        action_digest: &ActionDigest,
        manager: &mut CommandExecutionManager,
        re_max_queue_time: Option<Duration>,
        platform: &remote_execution::Platform,
        knobs: &ExecutorGlobalKnobs,
        persistent_worker: bool,
    ) -> anyhow::Result<ExecuteResponseOrCancelled> {
        use buck2_data::ReAfterAction;
        use buck2_data::ReBeforeAction;
        use buck2_data::ReExecute;
        use buck2_data::ReQueue;
        use buck2_data::ReUnknown;
        use buck2_data::ReWorkerDownload;
        use buck2_data::ReWorkerUpload;
        use buck2_data::re_stage;

        let action_key = if knobs.log_action_keys {
            metadata
                .action_history_info
                .as_ref()
                .map(|h| h.action_key.clone())
        } else {
            None
        };

        let re_use_case = metadata.use_case_id.clone();

        #[allow(clippy::large_enum_variant)]
        enum ResponseOrStateChange {
            Present(ExecuteWithProgressResponse),
            Cancelled(Cancelled),
        }

        /// Wait for either the ExecuteResponse to show up, or a stage change, within a span
        /// on the CommandExecutionManager.
        async fn wait_for_response_or_stage_change(
            receiver: &mut BoxStream<'static, anyhow::Result<ExecuteWithProgressResponse>>,
            previous_stage: Stage,
            previous_metadata: &Option<OperationMetadata>,
            report_stage: re_stage::Stage,
            manager: &mut CommandExecutionManager,
            queue_stats: &mut QueueStats,
            re_fallback_on_estimated_queue_time_exceeds_duration: Option<Duration>,
            re_cancel_on_estimated_queue_time_exceeds: Option<Duration>,
        ) -> anyhow::Result<ResponseOrStateChange> {
            executor_stage_async(
                buck2_data::ReStage {
                    stage: Some(report_stage),
                },
                async move {
                    loop {
                        let now = Instant::now();

                        let event = {
                            let next = futures::future::select(
                                manager.inner.liveliness_observer.while_alive(),
                                receiver.next(),
                            )
                            .await;

                            // Whenever time elapsed and we were previously in
                            // queued stage, we add the time that elapsed. If we
                            // just exited a queued stage, then we tracked how long
                            // we spent queued. If we are still in queue, we'll keep
                            // counting. This might not be as accurate as the RE
                            // logging but at least it works even if we get cancelled.
                            if matches!(previous_stage, Stage::QUEUED) {
                                queue_stats.cumulative_queue_duration += Instant::now() - now;
                            }

                            match next {
                                futures::future::Either::Left((_dead, _)) => {
                                    return Ok(ResponseOrStateChange::Cancelled(Cancelled {
                                        ..Default::default()
                                    }));
                                }
                                futures::future::Either::Right((event, _)) => match event {
                                    Some(event) => event,
                                    None => {
                                        return Err(anyhow::anyhow!(
                                            "RE execution did not yield a ExecuteResponse"
                                        ));
                                    }
                                },
                            }
                            .context("Error was returned on the stream by RE")?
                        };

                        // Check if we have a response or if the stage has changed
                        let stage_changed = event.stage != previous_stage;

                        // Check if queue information has changed (when in QUEUED stage)
                        let queue_info_changed = if matches!(event.stage, Stage::QUEUED)
                            && matches!(previous_stage, Stage::QUEUED)
                        {
                            // Check if the kind of task state has changed.
                            let previous_task_info = previous_metadata
                                .as_ref()
                                .and_then(|m| m.task_info.as_ref());
                            match (&event.metadata.task_info, previous_task_info) {
                                (Some(current_task_info), Some(previous_task_info)) => {
                                    std::mem::discriminant(&current_task_info.state)
                                        != std::mem::discriminant(&previous_task_info.state)
                                }
                                (None, None) => false,
                                _ => true,
                            }
                        } else {
                            false
                        };

                        if event.execute_response.is_some() || stage_changed || queue_info_changed {
                            return Ok(ResponseOrStateChange::Present(event));
                        }

                        if let Some(anticipated_queue_duration) =
                            anticipated_queue_duration(&event)?
                        {
                            if let Some(re_queue_threshold) =
                                re_cancel_on_estimated_queue_time_exceeds
                            {
                                if anticipated_queue_duration > re_queue_threshold {
                                    return Ok(ResponseOrStateChange::Cancelled(Cancelled {
                                        reason: Some(CancellationReason::ReQueueTimeout),
                                    }));
                                }
                            }

                            if let Some(re_acceptable_anticipated_queue_duration) =
                                re_fallback_on_estimated_queue_time_exceeds_duration
                            {
                                if anticipated_queue_duration
                                    > re_acceptable_anticipated_queue_duration
                                {
                                    manager.on_result_delayed();
                                }
                            }
                        }
                    }
                },
            )
            .await
        }

        fn get_queue_state(
            action_digest: String,
            use_case: String,
            operation_metadata: &Option<OperationMetadata>,
        ) -> re_stage::Stage {
            let queue_info = ReQueue {
                action_digest,
                use_case,
            };
            let state = operation_metadata
                .as_ref()
                .and_then(|m| m.task_info.as_ref())
                .map(|i| &i.state);
            match state {
                // Waiting to run, no extra info needed
                Some(TaskState::enqueued(..)) => re_stage::Stage::Queue(queue_info),
                Some(TaskState::waiting_on_reservation(..)) => re_stage::Stage::Queue(queue_info),
                Some(TaskState::waiting_for_gang_allocation(..)) => {
                    re_stage::Stage::Queue(queue_info)
                }
                // Useful info to display
                Some(TaskState::no_worker_available(..)) => {
                    re_stage::Stage::QueueNoWorkerAvailable(ReQueueNoWorkerAvailable {
                        queue_info: Some(queue_info),
                    })
                }
                Some(TaskState::cancelled(..)) => {
                    re_stage::Stage::QueueCancelled(ReQueueCancelled {
                        queue_info: Some(queue_info),
                    })
                }
                Some(TaskState::over_quota(over_quota)) => {
                    re_stage::Stage::QueueOverQuota(ReQueueOverQuota {
                        queue_info: Some(queue_info),
                        exceeded_quotas: over_quota
                            .exceeded_quotas
                            .iter()
                            .map(|q| q.name.clone())
                            .collect(),
                    })
                }
                Some(TaskState::acquiring_dependencies(..)) => {
                    re_stage::Stage::QueueAcquiringDependencies(ReQueueAcquiringDependencies {
                        queue_info: Some(queue_info),
                    })
                }
                // Unknown
                Some(TaskState::UnknownField(..)) => re_stage::Stage::Queue(queue_info),
                None => re_stage::Stage::Queue(queue_info),
            }
        }

        fn re_stage_from_exe_stage(
            stage: Stage,
            operation_metadata: &Option<OperationMetadata>,
            action_digest: String,
            platform: &remote_execution::Platform,
            action_key: &Option<String>,
            use_case: String,
            persistent_worker: bool,
        ) -> re_stage::Stage {
            match stage {
                Stage::QUEUED => get_queue_state(action_digest, use_case, operation_metadata),
                Stage::MATERIALIZING_INPUT => re_stage::Stage::WorkerDownload(ReWorkerDownload {
                    action_digest,
                    use_case,
                }),
                Stage::BEFORE_ACTION => {
                    re_stage::Stage::BeforeActionExecution(ReBeforeAction { action_digest })
                }
                Stage::EXECUTING => re_stage::Stage::Execute(ReExecute {
                    action_digest,
                    platform: Some(platform_to_proto(platform)),
                    action_key: action_key.clone(),
                    use_case,
                    persistent_worker,
                }),
                Stage::AFTER_ACTION => {
                    re_stage::Stage::AfterActionExecution(ReAfterAction { action_digest })
                }
                Stage::UPLOADING_OUTPUT => re_stage::Stage::WorkerUpload(ReWorkerUpload {
                    action_digest,
                    use_case,
                }),
                _ => {
                    tracing::debug!(
                        "Received unexpected RE stage {:#?} for action: {}",
                        stage,
                        action_digest
                    );
                    re_stage::Stage::Unknown(ReUnknown { action_digest })
                }
            }
        }

        // There are four possible outcomes, three of them failures:
        // 1. We get Err, which means we failed to communicate to RE.
        // 2. We get Ok(error.code != TCode::Ok), which means we have an RE-level failure
        // 3. We get Ok(action_result.exit_code != 0), which means the command failed
        // 4. We get Ok(0 everywhere), which means it worked

        // Obtain a stream of events from RE. If this fails then that is case #1 above so we
        // bail.
        let mut receiver = self
            .client()
            .get_execution_client()
            .execute_with_progress(metadata, request)
            // boxed() to segment the future
            .boxed()
            .await
            .context("Failed to start remote execution")?;

        // Now we wait until the ExecuteResponse shows up, and produce events accordingly. If
        // this doesn't give us an ExecuteResponse then this is case #1 again so we also fail.
        let action_digest_str = action_digest.to_string();
        let mut queue_stats = QueueStats::default();
        let mut exe_stage = Stage::QUEUED;
        let mut execution_started = ExecutionStarted::No;
        let mut operation_metadata = None;
        let mut log_stream_emitted = false;

        let re_fallback_on_estimated_queue_time_exceeds = knobs
            .re_fallback_on_estimated_queue_time_exceeds
            .or(re_max_queue_time);

        // Lets tests exercise the over-quota path without needing a throttled RE quota pool.
        let injected_exceeded_quotas = buck2_env!(
            "BUCK2_TEST_INJECTED_RE_EXCEEDED_QUOTAS",
            type = Vec<String>,
            converter = |v| Ok(v.split(' ').map(str::to_owned).collect()),
            applicability = testing
        )
        // Stringify the error because we can't deal with buck2_errors here
        .map_err(|e| anyhow::anyhow!(e))?;

        loop {
            let report_stage = match injected_exceeded_quotas {
                Some(exceeded_quotas) if exe_stage == Stage::QUEUED => {
                    re_stage::Stage::QueueOverQuota(ReQueueOverQuota {
                        queue_info: Some(ReQueue {
                            action_digest: action_digest_str.clone(),
                            use_case: re_use_case.clone(),
                        }),
                        exceeded_quotas: exceeded_quotas.clone(),
                    })
                }
                _ => re_stage_from_exe_stage(
                    exe_stage,
                    &operation_metadata,
                    action_digest_str.clone(),
                    platform,
                    &action_key,
                    re_use_case.clone(),
                    persistent_worker,
                ),
            };
            let progress_response = wait_for_response_or_stage_change(
                &mut receiver,
                exe_stage,
                &operation_metadata,
                report_stage,
                manager,
                &mut queue_stats,
                re_fallback_on_estimated_queue_time_exceeds,
                knobs.re_cancel_on_estimated_queue_time_exceeds,
            )
            .await?;

            let progress_response = match progress_response {
                ResponseOrStateChange::Present(r) => r,
                ResponseOrStateChange::Cancelled(c) => {
                    return Ok(ExecuteResponseOrCancelled::Cancelled(
                        c,
                        queue_stats,
                        execution_started,
                    ));
                }
            };

            // Return the result if we're done
            if let Some(execute_response) = progress_response.execute_response {
                return Ok(ExecuteResponseOrCancelled::Response(
                    ExecuteResponseWithQueueStats {
                        execute_response,
                        queue_stats,
                    },
                ));
            }

            // Change the stage
            exe_stage = progress_response.stage;
            if exe_stage == Stage::EXECUTING {
                execution_started = ExecutionStarted::Yes;
            }
            operation_metadata = Some(progress_response.metadata);

            // Emit log stream handles when first available
            if !log_stream_emitted {
                if let Some(ref meta) = operation_metadata {
                    if !meta.stdout_stream_name.is_empty() || !meta.stderr_stream_name.is_empty() {
                        log_stream_emitted = true;
                        get_dispatcher().instant_event(buck2_data::ReLogStreamAvailable {
                            action_digest: action_digest_str.clone(),
                            stdout_stream_name: meta.stdout_stream_name.clone(),
                            stderr_stream_name: meta.stderr_stream_name.clone(),
                            action_key: action_key.clone(),
                            use_case: re_use_case.clone(),
                        });
                    }
                }
            }
        }
    }

    pub async fn execute<'a>(
        &self,
        action_digest: ActionDigest,
        platform: &RE::Platform,
        dependencies: impl IntoIterator<Item = &'a RemoteExecutorDependency>,
        re_gang_workers: &[buck2_core::execution_types::executor_config::ReGangWorker],
        use_case: RemoteExecutorUseCase,
        identity: &ReActionIdentity<'_>,
        manager: &mut CommandExecutionManager,
        skip_cache_read: bool,
        skip_cache_write: bool,
        re_max_queue_time: Option<Duration>,
        re_resource_units: Option<i64>,
        knobs: &ExecutorGlobalKnobs,
        meta_internal_extra_params: &MetaInternalExtraParams,
        worker_tool_action_digest: Option<ActionDigest>,
        priority: Option<i32>,
    ) -> buck2_error::Result<ExecuteResponseOrCancelled> {
        let _exec_permit = self.exec_semaphore.acquire().await;

        #[cfg(not(fbcode_build))]
        let _unused = worker_tool_action_digest;
        #[cfg(not(fbcode_build))]
        let _unused = re_gang_workers;

        if buck2_env!("BUCK2_TEST_FAIL_RE_EXECUTE", bool, applicability = testing)? {
            return Err(test_re_error("Injected error", TCode::FAILED_PRECONDITION));
        }

        if buck2_env!(
            "BUCK2_TEST_FAIL_RE_RESOURCE_EXHAUSTED",
            bool,
            applicability = testing
        )? {
            return Err(test_re_error(
                "Injected RE resource exhausted error",
                TCode::RESOURCE_EXHAUSTED,
            ));
        }

        let induced_cache_miss = induced_cache_misses()
            .and_then(|misses| misses.get(&action_digest))
            .filter(|executed| !executed.load(std::sync::atomic::Ordering::Relaxed));

        let metadata = RemoteExecutionMetadata {
            platform: Some(re_platform(platform)),
            do_not_cache: skip_cache_write,
            buck_info: Some(BuckInfo {
                version: buck2_build_info::revision()
                    .map(|s| s.to_owned())
                    .unwrap_or_default(),
                build_id: identity.trace_id.to_string(),
                ..Default::default()
            }),
            respect_file_symlinks: Some(self.respect_file_symlinks),
            disable_cancel_on_disconnect: !meta_internal_extra_params.cancel_on_client_disconnect,
            ..use_case.metadata(Some(identity))
        };

        let request = ExecuteRequest {
            skip_cache_lookup: self.skip_remote_cache
                || skip_cache_read
                || induced_cache_miss.is_some(),
            execution_policy: Some(TExecutionPolicy {
                affinity_keys: vec![identity.affinity_key.clone()],
                // TODO: figure out what to do with priority from `meta_internal_extra_params`
                priority: priority
                    .or(meta_internal_extra_params.remote_execution_policy.priority)
                    .unwrap_or_default(),
                region_preference: meta_internal_extra_params
                    .remote_execution_policy
                    .region_preference
                    .clone()
                    .unwrap_or_default(),
                setup_preference_key: meta_internal_extra_params
                    .remote_execution_policy
                    .setup_preference_key
                    .clone()
                    .unwrap_or_default(),
                queue_timeout: meta_internal_extra_params
                    .remote_execution_policy
                    .queue_timeout_s
                    .map(|seconds| TDuration {
                        seconds: seconds.into(),
                        ..Default::default()
                    }),
                ..Default::default()
            }),
            action_digest: action_digest.to_re(),
            host_runtime_requirements: THostRuntimeRequirements {
                platform: re_platform(platform),
                host_resource_requirements: THostResourceRequirements {
                    input_files_bytes: identity.paths.input_files_bytes() as i64,
                    resource_units: re_resource_units.unwrap_or_default(),
                    ..Default::default()
                },
                dependencies: dependencies
                    .into_iter()
                    .map(|dep| TDependency {
                        smc_tier: dep.smc_tier.clone(),
                        id: dep.id.clone(),
                        ..Default::default()
                    })
                    .collect(),
                #[cfg(fbcode_build)]
                worker_tool_action_digest: worker_tool_action_digest
                    .map(|d| d.to_re())
                    .unwrap_or(Default::default()),
                ..Default::default()
            },
            #[cfg(fbcode_build)]
            gang: if let Some(gang) = meta_internal_extra_params.gang.as_ref() {
                let properties: Vec<_> = gang
                    .capabilities
                    .iter()
                    .map(|(k, v)| remote_execution::TProperty {
                        name: k.clone(),
                        value: v.clone(),
                        ..Default::default()
                    })
                    .collect();

                let member_spec = remote_execution::GangMember {
                    host_runtime_requirements: THostRuntimeRequirements {
                        platform: remote_execution::TPlatform {
                            properties,
                            ..Default::default()
                        },
                        host_resource_requirements: THostResourceRequirements {
                            input_files_bytes: identity.paths.input_files_bytes() as i64,
                            resource_units: gang.resource_units.map(i64::from).unwrap_or_default(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let locality = gang.locality.map(|l| {
                    use buck2_core::execution_types::executor_config::ReGangLocality;
                    match l {
                        ReGangLocality::Unspecified => {
                            remote_execution::LocalityConstraint::UNSPECIFIED
                        }
                        ReGangLocality::Region => remote_execution::LocalityConstraint::REGION,
                        ReGangLocality::Datacenter => {
                            remote_execution::LocalityConstraint::DATACENTER
                        }
                        ReGangLocality::NetworkDomain => {
                            remote_execution::LocalityConstraint::NETWORK_DOMAIN
                        }
                        ReGangLocality::Rack => remote_execution::LocalityConstraint::RACK,
                    }
                });

                Some(remote_execution::GangSpecification {
                    workers_spec: remote_execution::GangWorkersSpec::constrained_spec(
                        remote_execution::ConstrainedGangSpec {
                            num_workers: gang.num_of_workers,
                            member_spec,
                            locality,
                            num_sub_groups: gang.num_sub_groups.unwrap_or(1),
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                })
            } else if re_gang_workers.is_empty() {
                None
            } else {
                let mut gang_members = Vec::with_capacity(re_gang_workers.len());
                for worker in re_gang_workers.iter() {
                    let properties: Vec<_> = worker
                        .capabilities
                        .iter()
                        .map(|(k, v)| remote_execution::TProperty {
                            name: k.clone(),
                            value: v.clone(),
                            ..Default::default()
                        })
                        .collect();

                    gang_members.push(remote_execution::GangMember {
                        host_runtime_requirements: THostRuntimeRequirements {
                            platform: remote_execution::TPlatform {
                                properties,
                                ..Default::default()
                            },
                            host_resource_requirements: THostResourceRequirements {
                                input_files_bytes: identity.paths.input_files_bytes() as i64,
                                ..Default::default()
                            },
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                }

                Some(remote_execution::GangSpecification {
                    workers_spec: remote_execution::GangWorkersSpec::enumerated_spec(
                        remote_execution::EnumeratedGangSpec {
                            workers: gang_members,
                            ..Default::default()
                        },
                    ),
                    ..Default::default()
                })
            },
            ..Default::default()
        };
        let re_action = format!("Execute with digest {}", action_digest);
        let res = with_error_handler(
            re_action.as_str(),
            self.get_session_id(),
            self.execute_impl(
                &metadata,
                request,
                &action_digest,
                manager,
                re_max_queue_time,
                platform,
                knobs,
                worker_tool_action_digest.is_some(),
            )
            .await,
        );

        if let Some(induced_cache_miss) = induced_cache_miss {
            induced_cache_miss.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        let trace = match &res {
            Ok(ExecuteResponseOrCancelled::Response(r)) => {
                // If the action was served from cache or deduplicated, we don't treat it as a cache
                // hit instead of an execution - it'll be treated as an execution on the side of the
                // thing that it got a hit or deduplicated against.
                let was_not_actually_executed = r.origin() != RemoteExecutionOrigin::Executed;
                let event = if was_not_actually_executed {
                    buck2_data::action_digest_trace::ActionDigestTraceEvent::CacheHit
                } else {
                    buck2_data::action_digest_trace::ActionDigestTraceEvent::RemoteExecution
                };
                let (storage_cost_bytes, compute_cost_ms) = if !was_not_actually_executed {
                    let (storage, compute) = action_result_costs(&r.execute_response.action_result);
                    (Some(storage), Some(compute))
                } else {
                    (None, None)
                };
                Some((
                    event,
                    Some(r.ttl()),
                    "completed",
                    storage_cost_bytes,
                    compute_cost_ms,
                ))
            }
            // RE drops a cancelled execution that had not started. A started one already ran on RE,
            // whether RE then finishes or stops it, so trace it as an execution.
            Ok(ExecuteResponseOrCancelled::Cancelled(_, _, ExecutionStarted::Yes)) => Some((
                buck2_data::action_digest_trace::ActionDigestTraceEvent::RemoteExecution,
                None,
                "cancelled-after-execution-started",
                None,
                None,
            )),
            Ok(ExecuteResponseOrCancelled::Cancelled(_, _, ExecutionStarted::No)) => None,
            Err(_) => None,
        };
        if let Some((event, ttl, subtype, storage_cost_bytes, compute_cost_ms)) = trace {
            trace_action_digest(
                &action_digest,
                ttl,
                use_case,
                event,
                Some(subtype),
                storage_cost_bytes,
                compute_cost_ms,
            );
        }

        res
    }

    /// Fetches a list of digests from the CAS and casts them to Tree objects.
    /// If fetching or decoding fails for one or more digests, returns an Err.
    async fn download_typed_blobs<T: Message + Default>(
        &self,
        identity: Option<&ReActionIdentity<'_>>,
        digests: Vec<TDigest>,
        use_case: RemoteExecutorUseCase,
    ) -> buck2_error::Result<(Vec<T>, TLocalCacheStats)> {
        if digests.is_empty() {
            return Ok((Vec::new(), TLocalCacheStats::default()));
        }
        if buck2_env!(
            "BUCK2_TEST_FAIL_RE_DOWNLOADS",
            bool,
            applicability = testing
        )? {
            return Err(test_re_error_with_group(
                "Injected error",
                TCode::NOT_FOUND,
                RE::TCodeReasonGroup::DIGEST_NOT_FOUND,
            ));
        }
        let expected_blobs = digests.len();
        let download = {
            let metadata = use_case.metadata(identity);
            self.client().get_cas_client().download(
                &metadata,
                DownloadRequest {
                    inlined_digests: Some(digests),
                    ..Default::default()
                },
            )
        };
        let response = with_error_handler(
            "download_typed_blobs",
            self.get_session_id(),
            download.await,
        )?;

        let mut blobs: Vec<T> = Vec::with_capacity(expected_blobs);
        if let Some(ds) = response.inlined_blobs {
            for d in ds {
                blobs.push(
                    Message::decode(d.blob.as_slice()).with_buck_error_context(|| {
                        format!("Failed to Protobuf decode tree at `{}`", d.digest)
                    })?,
                );
            }
        }

        // This shouldn't happen, but we can't just assume the CAS won't ever break
        if blobs.len() != expected_blobs {
            return Err(buck2_error!(
                buck2_error::ErrorTag::CasBlobCountMismatch,
                "CAS client returned fewer blobs than expected."
            ));
        }

        Ok((blobs, response.local_cache_stats))
    }

    pub async fn download_blob(
        &self,
        digest: &TDigest,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<(Vec<u8>, TLocalCacheStats)> {
        let re_action = format!("download_blob for digest {digest}");
        let download = self.client().get_cas_client().download(
            metadata,
            DownloadRequest {
                inlined_digests: Some(vec![digest.clone()]),
                ..Default::default()
            },
        );
        let response =
            with_error_handler(re_action.as_str(), self.get_session_id(), download.await)?;

        response
            .inlined_blobs
            .into_iter()
            .flat_map(|blobs| blobs.into_iter())
            .next()
            .map(|blob| (blob.blob, response.local_cache_stats))
            .with_internal_error(|| format!("No digest was returned in request for {digest}"))
    }

    pub async fn upload_blob(
        &self,
        blob: InlinedBlobWithDigest,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<TDigest> {
        let upload = self
            .client()
            .upload_blob_with_digest(blob.blob, blob.digest, metadata);
        with_error_handler("upload_blob", self.get_session_id(), upload.await)
    }

    async fn materialize_files(
        &self,
        files: Vec<NamedDigestWithPermissions>,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<TLocalCacheStats> {
        if buck2_env!(
            "BUCK2_TEST_FAIL_RE_DOWNLOADS",
            bool,
            applicability = testing
        )? {
            return Err(test_re_error_with_group(
                "Injected error",
                TCode::NOT_FOUND,
                RE::TCodeReasonGroup::DIGEST_NOT_FOUND,
            ));
        }

        let futs = chunks(files, self.download_chunk_size).map(|chunk| async move {
            let _permit = self
                .download_files_semapore
                .acquire_many(
                    chunk
                        .len()
                        .try_into()
                        .buck_error_context("chunk is too large")?,
                )
                .await
                .buck_error_context("Failed to acquire download_files_semapore")?;

            let download = self.client().get_cas_client().download(
                metadata,
                DownloadRequest {
                    file_digests: Some(chunk),
                    ..Default::default()
                },
            );
            let response =
                with_error_handler("materialize_files", self.get_session_id(), download.await)?;

            buck2_error::Ok(response.local_cache_stats)
        });

        let stat = buck2_util::future::try_join_all(futs).await?.iter().fold(
            TLocalCacheStats::default(),
            |acc, x| TLocalCacheStats {
                hits_files: acc.hits_files + x.hits_files,
                hits_bytes: acc.hits_bytes + x.hits_bytes,
                misses_files: acc.misses_files + x.misses_files,
                misses_bytes: acc.misses_bytes + x.misses_bytes,
                ..Default::default()
            },
        );

        Ok(stat)
    }

    async fn get_digests_ttl(
        &self,
        digests: Vec<TDigest>,
        metadata: &RemoteExecutionMetadata,
        is_for_upload: bool,
    ) -> buck2_error::Result<GetDigestsTtlResponse> {
        with_error_handler(
            "get_digests_ttl",
            self.get_session_id(),
            self.client()
                .get_cas_client()
                .get_digests_ttl(
                    metadata,
                    GetDigestsTtlRequest {
                        digests,
                        is_for_upload: Some(is_for_upload),
                        ..Default::default()
                    },
                )
                .await,
        )
    }

    async fn extend_digest_ttl(
        &self,
        digests: Vec<TDigest>,
        ttl: Duration,
        metadata: &RemoteExecutionMetadata,
    ) -> buck2_error::Result<()> {
        // TODO(arr): use batch API from RE when it becomes available
        with_error_handler(
            "extend_digest_ttl",
            self.get_session_id(),
            self.client()
                .get_cas_client()
                .extend_digest_ttl(
                    metadata,
                    ExtendDigestsTtlRequest {
                        digests,
                        ttl: ttl.as_secs() as i64,
                        ..Default::default()
                    },
                )
                .await,
        )?;
        Ok(())
    }

    async fn write_action_result(
        &self,
        digest: ActionDigest,
        result: &mut TActionResult2,
        use_case: RemoteExecutorUseCase,
        platform: &RE::Platform,
        write_type: ActionCacheWriteType,
    ) -> buck2_error::Result<WriteActionResultResponse> {
        let (write, storage_cost_bytes, compute_cost_ms) = {
            let (storage_cost_bytes, compute_cost_ms) =
                if matches!(write_type, ActionCacheWriteType::LocalCacheUpload) {
                    let (storage, compute) = action_result_costs(result);
                    (Some(storage), Some(compute))
                } else {
                    (None, None)
                };

            // The request needs to own the result; lend it for serialization and hand it back.
            let request = WriteActionResultRequest {
                action_digest: digest.to_re(),
                action_result: std::mem::take(result),
                platform: Some(re_platform(platform)),
                ..Default::default()
            };
            let attributes =
                BTreeMap::from([("write_type".to_owned(), write_type.as_str().to_owned())]);
            let metadata = RemoteExecutionMetadata {
                platform: Some(re_platform(platform)),
                client_context: Some(TClientContextMetadata {
                    attributes,
                    ..Default::default()
                }),
                ..use_case.metadata(None)
            };
            let write = self
                .client()
                .get_action_cache_client()
                .write_action_result(&metadata, &request);
            *result = request.action_result;
            (write, storage_cost_bytes, compute_cost_ms)
        };
        let response =
            with_error_handler("write_action_result", self.get_session_id(), write.await)?;

        trace_action_digest(
            &digest,
            Some(response.ttl_seconds),
            use_case,
            buck2_data::action_digest_trace::ActionDigestTraceEvent::CacheUpload,
            Some(write_type.as_str()),
            storage_cost_bytes,
            compute_cost_ms,
        );

        Ok(response)
    }
}

#[cfg(fbcode_build)] // Relies on fbcode future sizes
buck2_util::size_assert::words_of_async_fn_future!(
    RemoteExecutionClientImpl::write_action_result,
    (_, _, _, _, _, _),
    ~38
);

#[cfg(fbcode_build)] // Relies on fbcode future sizes
buck2_util::size_assert::words_of_async_fn_future!(
    RemoteExecutionClientImpl::get_digests_ttl,
    (_, _, _, _),
    ~38
);

/// Drop the REClient on a blocking thread. The REClient destructor does a blocking wait on async
/// calls (it tells the server to cancel its calls, but it waits for an ack), so we shouldn't drop
/// it on a runtime thread.
impl Drop for RemoteExecutionClientImpl {
    fn drop(&mut self) {
        tracing::info!("Dropping RE client");
        let client = self.client.take();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn_blocking(move || {
                drop(client);
                tracing::info!("Dropped RE client");
            });
        }
    }
}

fn chunks<T>(v: Vec<T>, chunk_size: usize) -> impl ExactSizeIterator<Item = Vec<T>> {
    if !v.is_empty() && v.len() <= chunk_size {
        return Either::Left(std::iter::once(v));
    }

    let chunks = v
        .into_iter()
        .chunks(chunk_size)
        .into_iter()
        .map(|c| c.collect::<Vec<_>>())
        .collect::<Vec<_>>();

    Either::Right(chunks.into_iter())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunks_skips() {
        assert_eq!(chunks(Vec::<usize>::new(), 1).next(), None);
    }

    #[test]
    fn test_chunks_reuses() {
        let v = vec![1, 2, 3];
        let addr = v.as_ptr() as usize;
        let mut it = chunks(v, 3);
        let first = it.next().unwrap();
        assert_eq!(first.as_ptr() as usize, addr);
        assert_eq!(it.next(), None);
    }

    #[test]
    fn test_chunks_splits() {
        let v = vec![1, 2, 3];
        let mut it = chunks(v, 2);
        assert_eq!(it.next(), Some(vec![1, 2]));
        assert_eq!(it.next(), Some(vec![3]));
        assert_eq!(it.next(), None);
    }
}
