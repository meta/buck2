/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

// This triggers on Arc<Arc<...>>, but we do that here for lifetime/ownership reasons
#![allow(clippy::redundant_allocation)]

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::Weak;
use std::time::Duration;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_common::file_ops::metadata::TrackedFileDigest;
use buck2_core::async_once_cell::AsyncOnceCell;
use buck2_core::buck2_env;
use buck2_core::execution_types::executor_config::MetaInternalExtraParams;
use buck2_core::execution_types::executor_config::RemoteExecutorDependency;
use buck2_core::execution_types::executor_config::RemoteExecutorUseCase;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePath;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::conversion::from_any_with_tag;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_re_configuration::RemoteExecutionStaticMetadata;
use buck2_re_configuration::RemoteExecutionStaticMetadataImpl;
use dupe::Dupe;
use fbinit::FacebookInit;
use futures::FutureExt;
use prost::Message;
use remote_execution as RE;
use remote_execution::ActionResultResponse;
use remote_execution::InlinedBlobWithDigest;
use remote_execution::NamedDigest;
use remote_execution::NamedDigestWithPermissions;
use remote_execution::RemoteExecutionMetadata;
use remote_execution::TActionResult2;
use remote_execution::TDigest;
use remote_execution::WriteActionResultResponse;

use crate::directory::ActionImmutableDirectory;
use crate::execute::action_digest::ActionDigest;
use crate::execute::blobs::ActionBlobs;
use crate::execute::manager::CommandExecutionManager;
use crate::knobs::ExecutorGlobalKnobs;
use crate::materialize::materializer::CasDownloadInfo;
use crate::materialize::materializer::Materializer;
use crate::re::action_identity::ReActionIdentity;
use crate::re::client::ActionCacheWriteType;
use crate::re::client::ExecuteResponseOrCancelled;
use crate::re::client::RemoteExecutionClient;
use crate::re::metadata::RemoteExecutionMetadataExt;
use crate::re::presence::CasPresence;
use crate::re::presence::NegativeCache;
use crate::re::presence::Presence;
use crate::re::re_get_session_id::ReGetSessionId;
use crate::re::stats::RemoteExecutionClientStats;
use crate::re::uploader::UploadStats;

/// Lifetime management of the Remote Execution connection (i.e. the RemoteExecutionClient).
///
/// The [ReConnectionManager] ensures that there is only ever a single remote execution connection. It
/// vends out a [ReConnectionHandle] on calls to [ReConnectionManager::get_re_connection()].
///
/// There will only be at most a single RE connection during the lifetime of an [ReConnectionHandle], the
/// connection will not be closed as long as any handle still exists. The [ReConnectionHandle] then vends
/// out a [ManagedRemoteExecutionClient].
///
/// The [ManagedRemoteExecutionClient] has all the RE functions on it. The first time one of these is
/// called will be when the RE connection is actually made, if no calls to RE are made, we don't create
/// a connection.
///
/// When a [ManagedRemoteExecutionClient] is used, if it's source [ReConnectionHandle] still exists then
/// the calls will go to the underlying RE client (and making the connection if this is the first call). If,
/// however, the [ReConnectionHandle] has been dropped the calls will fail (even if the connection is still
/// alive from other handles).
///
/// When all existing [ReConnectionHandle] (for a particular [ReConnectionManager]) are dropped, the
/// underlying connection will be closed.
///
/// This approach allows us to ensure that all RE interactions for a particular buck command use the
/// same RE session. Concurrent commands will share an RE session.

#[derive(Clone, Allocative)]
pub struct RemoteExecutionConfig {
    #[allocative(skip)] // TODO(nga): implement in `allocative`.
    pub fb: FacebookInit,
    /// whether to skip the cache when performing RE
    pub skip_remote_cache: bool,
    /// number of retries when attempting the initial RE connection
    pub connection_retries: usize,
    pub static_metadata: Arc<RemoteExecutionStaticMetadata>,
    pub logs_dir_path: Option<AbsNormPathBuf>,
    pub buck_out_path: AbsNormPathBuf,
    /// Whether Buck is running in paranoid mode.
    pub is_paranoid_mode: bool,
}

impl RemoteExecutionConfig {
    async fn connect_now(&self) -> buck2_error::Result<RemoteExecutionClient> {
        RemoteExecutionClient::new_retry(self).await
    }
}

pub trait ReConnectionObserver: Allocative + 'static + Send + Sync {
    fn session_created(&self, client: &RemoteExecutionClient);
}

#[derive(Allocative)]
struct LazyRemoteExecutionClient {
    client: AsyncOnceCell<buck2_error::Result<RemoteExecutionClient>>,
    observers: Mutex<Vec<Weak<dyn ReConnectionObserver>>>,
    config: RemoteExecutionConfig,
}

impl LazyRemoteExecutionClient {
    fn new(config: RemoteExecutionConfig) -> LazyRemoteExecutionClient {
        Self {
            client: AsyncOnceCell::new(),
            observers: Mutex::new(Vec::new()),
            config,
        }
    }

    /// Apply F to the client contained in this LazyRemoteExecutionClient if and only if there is a
    /// valid client.
    fn with_client<F, T>(&self, f: F) -> Option<T>
    where
        F: FnOnce(&RemoteExecutionClient) -> T,
    {
        match self.client.get().as_ref() {
            Some(Ok(client)) => Some(f(client)),
            _ => None,
        }
    }

    async fn get(&self) -> buck2_error::Result<&RemoteExecutionClient> {
        // The future from self.init() is large and very rarely required. We want
        // to ensure that (1) it doesn't contribute to the size of the get() future
        // (which is used for basically every call) and (2) isn't even allocated on
        // the fast path.
        // To do this, we .boxed() it and wrap it in an async block that only needs to
        // hold the &self (until it's first polled).
        let init_fut = async move { self.init().boxed().await };
        match self.client.get_or_init(init_fut).await {
            Ok(v) => Ok(v),
            Err(e) => Err(e.dupe()),
        }
    }

    async fn init(&self) -> buck2_error::Result<RemoteExecutionClient> {
        let client = self.config.connect_now().await?;

        let mut observers = self.observers.lock().unwrap();
        let owned = std::mem::take(&mut *observers);
        // session_created is the only thing on observer right now, so we can just throw them all out at this point.
        for o in owned.into_iter() {
            if let Some(o) = o.upgrade() {
                o.session_created(&client);
            }
        }
        Ok(client)
    }

    // Adds an observer. This observer will immediately get a session_created() call if we are already connected.
    fn observe(&self, observer: &Arc<dyn ReConnectionObserver>) {
        // lock observers so we don't race with the notifications in init()
        let mut observers = self.observers.lock().unwrap();
        match self.client.get() {
            Some(Ok(client)) => {
                observer.session_created(client);
            }
            Some(_) => {
                // ignored
            }
            _ => {
                observers.push(Arc::downgrade(observer));
            }
        }
    }
}

/// The main manager for the RE connections
#[derive(Allocative)]
pub struct ReConnectionManager {
    // We hold a single Weak to the lazy client. ReConnectionHandle will hold a strong Arc to the same. Once the
    // last ReConnectionHandle is dropped, the client we point to will be dropped and we'll create a new one for
    // the next ReConnectionHandle.
    data: RwLock<Weak<LazyRemoteExecutionClient>>,
    config: RemoteExecutionConfig,
}

impl ReConnectionManager {
    pub fn new(
        fb: FacebookInit,
        skip_remote_cache: bool,
        connection_retries: usize,
        static_metadata: Arc<RemoteExecutionStaticMetadata>,
        logs_dir_path: Option<AbsNormPathBuf>,
        buck_out_path: AbsNormPathBuf,
        is_paranoid_mode: bool,
    ) -> Self {
        Self {
            data: RwLock::new(Weak::new()),
            config: RemoteExecutionConfig {
                fb,
                skip_remote_cache,
                connection_retries,
                static_metadata,
                logs_dir_path,
                buck_out_path,
                is_paranoid_mode,
            },
        }
    }

    /// Whether there is a CAS to talk to at all; see `RemoteExecutionStaticMetadataImpl`.
    pub fn cas_configured(&self) -> bool {
        self.config.static_metadata.cas_configured()
    }

    /// Gets a new guard that holds a RE connection open
    pub fn get_re_connection(&self) -> ReConnectionHandle {
        ReConnectionHandle::new(self.get_client_handle())
    }

    fn get_client_handle(&self) -> Arc<LazyRemoteExecutionClient> {
        if let Some(conn) = self.data.read().unwrap().upgrade() {
            return conn;
        }

        let mut conn = self.data.write().unwrap();
        match conn.upgrade() {
            None => {
                let new_connection = Arc::new(LazyRemoteExecutionClient::new(self.config.clone()));
                *conn = Arc::downgrade(&new_connection);
                new_connection
            }
            Some(conn) => conn,
        }
    }

    pub fn get_network_stats(&self) -> buck2_error::Result<RemoteExecutionClientStats> {
        let client_stats = RE::get_network_stats()
            .map_err(|e| from_any_with_tag(e, buck2_error::ErrorTag::Tier0))
            .buck_error_context("Error getting RE network stats")?;

        // Those two fields come from RE and are always available.
        let mut res = RemoteExecutionClientStats {
            uploaded: client_stats.uploaded as _,
            downloaded: client_stats.downloaded as _,
            ..Default::default()
        };

        res.upload_stats
            .fill_from_re_client_metrics(&client_stats.upload_storage_stats);
        res.download_stats
            .fill_from_re_client_metrics(&client_stats.download_storage_stats);
        // Process-wide, so it is available whether or not a connection is open.
        res.presence = CasPresence::global_stats().unwrap_or_default();

        // The rest of the fields are known to be their default value if we don't have a client, so
        // we ask the client to fill them iff we have one.
        let conn = self.data.read().unwrap().upgrade();
        if let Some(conn) = &conn {
            conn.with_client(|client| client.fill_network_stats(&mut res));
        }

        Ok(res)
    }
}

#[async_trait]
impl ReGetSessionId for ReConnectionManager {
    async fn get_session_id(&self) -> buck2_error::Result<String> {
        self.get_re_connection().get_client().get_session_id().await
    }
}

/// The guard that holds the RE connection open. This can outlive the main manager
/// The client obtained from this handle will only be valid while this handle is held.
pub struct ReConnectionHandle {
    /// here we hold an `Arc<Arc>` so that we can hand out a `Weak<Arc>` so that upon the handle
    /// being dropped, the clients will lose the ability to access connection
    // TODO(cjhopman): While we vend out Weak<Arc> with this, due to the way this is stored/used on
    // the dice graph we are guaranteed that there's an Arc that outlives all the Weak from it. That
    // kind of defeats the purpose of this.
    connection: Arc<Arc<LazyRemoteExecutionClient>>,
    // We use a similar Arc/Weak to ensure that the observer only lives as long as the connection
    // handle. Otherwise it would be possible to observer a session create attached to one command
    // after that command ended. An alternative would be to register/deregister the connection
    // handle itself as an observer on the lazy client, but that doesn't seem any simpler.
    observer: Option<Arc<dyn ReConnectionObserver>>,
}

impl ReConnectionHandle {
    fn new(connection: Arc<LazyRemoteExecutionClient>) -> Self {
        Self {
            connection: Arc::new(connection),
            observer: None,
        }
    }

    /// Sets the connection observer. This will drop the previous observer if there is one.
    pub fn set_observer(&mut self, observer: Arc<dyn ReConnectionObserver>) {
        // We store it just to give it this handle's lifetime.
        self.connection.observe(&observer);
        self.observer = Some(observer);
    }

    /// gets a client that is tied to the scope of this guard
    pub fn get_client(&self) -> UnconfiguredRemoteExecutionClient {
        UnconfiguredRemoteExecutionClient {
            data: Arc::downgrade(&self.connection),
        }
    }

    pub fn release_temporary_memory(&self) -> buck2_error::Result<()> {
        if let Some(result) = self
            .connection
            .with_client(RemoteExecutionClient::release_temporary_memory)
        {
            result?;
        }
        Ok(())
    }
}

#[derive(Clone, Dupe)]
pub struct UnconfiguredRemoteExecutionClient {
    data: Weak<Arc<LazyRemoteExecutionClient>>,
}

#[derive(Clone, Dupe)]
pub struct ManagedRemoteExecutionClient {
    inner: UnconfiguredRemoteExecutionClient,
    pub use_case: RemoteExecutorUseCase,
    /// Built once per client; calls borrow it if they aren't associated with an action.
    no_action_metadata: Arc<RemoteExecutionMetadata>,
}

impl UnconfiguredRemoteExecutionClient {
    pub fn with_use_case(self, use_case: RemoteExecutorUseCase) -> ManagedRemoteExecutionClient {
        ManagedRemoteExecutionClient {
            inner: self,
            use_case,
            no_action_metadata: Arc::new(use_case.metadata(None)),
        }
    }

    fn lock(&self) -> buck2_error::Result<Arc<Arc<LazyRemoteExecutionClient>>> {
        self.data
            .upgrade()
            .internal_error("Internal error: the underlying RE connection has terminated because the corresponding guard has been dropped.")
    }

    pub async fn get_session_id(&self) -> buck2_error::Result<String> {
        let session_id = self.lock()?.get().await?.get_session_id().to_owned();
        Ok(session_id)
    }

    // Construct a dummy UnconfiguredRemoteExecutionClient that won't actually work. This is only
    // remotely useful in tests.
    pub fn testing_new_dummy() -> Self {
        Self { data: Weak::new() }
    }
}

impl ManagedRemoteExecutionClient {
    pub async fn get_session_id(&self) -> buck2_error::Result<String> {
        self.inner.get_session_id().await
    }

    fn lock(&self) -> buck2_error::Result<Arc<Arc<LazyRemoteExecutionClient>>> {
        self.inner.lock()
    }

    pub async fn action_cache(
        &self,
        action_digest: ActionDigest,
        platform: &RE::Platform,
        identity: Option<&ReActionIdentity<'_>>,
    ) -> buck2_error::Result<Option<ActionResultResponse>> {
        // `Ok(None)` is an outright miss; `Err` is a real failure. The caller
        // decides whether to degrade failures into misses.
        self.lock()?
            .get()
            .await?
            .action_cache(
                action_digest,
                &self.no_action_metadata,
                self.use_case,
                platform,
                identity,
            )
            .await
    }

    pub async fn upload(
        &self,
        fs: &ProjectRoot,
        materializer: &dyn Materializer,
        blobs: &ActionBlobs,
        dir_path: &ProjectRelativePath,
        input_dir: &ActionImmutableDirectory,
        identity: Option<&ReActionIdentity<'_>>,
    ) -> buck2_error::Result<UploadStats> {
        self.lock()?
            .get()
            .await?
            .upload(
                fs,
                materializer,
                blobs,
                dir_path,
                input_dir,
                self.use_case,
                identity,
            )
            .await
    }

    pub async fn upload_blobs(&self, blobs: &ActionBlobs) -> buck2_error::Result<UploadStats> {
        self.lock()?
            .get()
            .await?
            .upload_blobs(blobs, self.use_case)
            .await
    }

    pub async fn upload_files_and_directories(
        &self,
        files_with_digest: Vec<NamedDigest>,
        directories: Vec<remote_execution::Path>,
        inlined_blobs_with_digest: Vec<InlinedBlobWithDigest>,
        identity: Option<&ReActionIdentity<'_>>,
    ) -> buck2_error::Result<()> {
        self.lock()?
            .get()
            .await?
            .upload_files_and_directories(
                files_with_digest,
                directories,
                inlined_blobs_with_digest,
                &self.no_action_metadata,
                identity,
            )
            .await
    }

    pub async fn execute<'a>(
        &self,
        action_digest: ActionDigest,
        platform: &RE::Platform,
        dependencies: impl IntoIterator<Item = &'a RemoteExecutorDependency>,
        re_gang_workers: &[buck2_core::execution_types::executor_config::ReGangWorker],
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
        self.lock()?
            .get()
            .await?
            .execute(
                action_digest,
                platform,
                dependencies,
                re_gang_workers,
                self.use_case,
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
            )
            .await
    }

    pub async fn materialize_files(
        &self,
        files: Vec<NamedDigestWithPermissions>,
        info: &CasDownloadInfo,
    ) -> buck2_error::Result<()> {
        let result = self
            .lock()?
            .get()
            .await?
            .materialize_files(files, &self.no_action_metadata)
            .await;
        self.classify_cas_result(info, result)
    }

    pub async fn download_typed_blobs<T: Message + Default>(
        &self,
        identity: Option<&ReActionIdentity<'_>>,
        digests: Vec<TDigest>,
        info: &CasDownloadInfo,
    ) -> buck2_error::Result<Vec<T>> {
        let result = self
            .lock()?
            .get()
            .await?
            .download_typed_blobs(identity, digests, self.use_case)
            .await;
        self.classify_cas_result(info, result)
    }

    pub async fn download_blob(&self, digest: &TDigest) -> buck2_error::Result<Vec<u8>> {
        self.lock()?
            .get()
            .await?
            .download_blob(digest, &self.no_action_metadata)
            .await
    }

    pub async fn upload_blob(&self, blob: InlinedBlobWithDigest) -> buck2_error::Result<TDigest> {
        self.lock()?
            .get()
            .await?
            .upload_blob(blob, &self.no_action_metadata)
            .await
    }

    /// See [`RemoteExecutionClient::check_presence`]; misses and failures are classified per
    /// `info`, whose use case must be this client's.
    pub async fn check_presence(
        &self,
        digests: Vec<TrackedFileDigest>,
        negative_cache: NegativeCache,
        info: &CasDownloadInfo,
    ) -> buck2_error::Result<Vec<Presence>> {
        let result = self
            .lock()?
            .get()
            .await?
            .check_presence(self.use_case, digests, negative_cache)
            .await;
        self.classify_cas_result(info, result)
    }

    pub async fn get_digest_expirations(
        &self,
        digests: Vec<TDigest>,
        info: &CasDownloadInfo,
    ) -> buck2_error::Result<Vec<(TDigest, jiff::Timestamp)>> {
        let result = self
            .lock()?
            .get()
            .await?
            .get_digest_expirations(digests, &self.no_action_metadata)
            .await;
        self.classify_cas_result(info, result)
    }

    pub async fn extend_digest_ttl(
        &self,
        digests: Vec<TDigest>,
        ttl: Duration,
        info: &CasDownloadInfo,
    ) -> buck2_error::Result<()> {
        let result = self
            .lock()?
            .get()
            .await?
            .extend_digest_ttl(digests, ttl, &self.no_action_metadata)
            .await;
        self.classify_cas_result(info, result)
    }

    pub async fn write_action_result(
        &self,
        digest: ActionDigest,
        result: &mut TActionResult2,
        identity: Option<&ReActionIdentity<'_>>,
        platform: &RE::Platform,
        write_type: ActionCacheWriteType,
    ) -> buck2_error::Result<WriteActionResultResponse> {
        if buck2_env!(
            "BUCK2_TEST_SKIP_ACTION_CACHE_WRITE",
            bool,
            applicability = testing
        )? {
            Ok(WriteActionResultResponse {
                actual_action_result: result.clone(),
                ..Default::default()
            })
        } else {
            self.lock()?
                .get()
                .await?
                .write_action_result(
                    digest,
                    result,
                    self.use_case,
                    identity,
                    platform,
                    write_type,
                )
                .await
        }
    }

    fn classify_cas_result<T>(
        &self,
        info: &CasDownloadInfo,
        result: buck2_error::Result<T>,
    ) -> buck2_error::Result<T> {
        debug_assert_eq!(self.use_case, info.re_use_case);
        info.classify_result(result)
    }
}
