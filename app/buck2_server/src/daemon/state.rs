/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future::Future;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use allocative::Allocative;
use buck2_build_api::spawner::BuckSpawner;
use buck2_cli_proto::ClientContext;
use buck2_cli_proto::unstable_dice_dump_request::DiceDumpFormat;
use buck2_common::cancellation::CancellationScope;
use buck2_common::cas_digest::DigestAlgorithm;
use buck2_common::cas_digest::DigestAlgorithmFamily;
use buck2_common::ignores::ignore_set::IgnoreSet;
use buck2_common::ignores::ignore_set::compute_ignore_set;
use buck2_common::init::DaemonStartupConfig;
use buck2_common::init::SystemWarningConfig;
use buck2_common::init::Timeout;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_common::invocation_paths::TenantDiskLock;
use buck2_common::invocation_paths::TenantPaths;
use buck2_common::io::IoProvider;
use buck2_common::io::trace::TracingIoProvider;
use buck2_common::legacy_configs::cells::BuckConfigBasedCells;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::legacy_configs::key::BuckconfigKeyRef;
use buck2_common::legacy_configs::parse_buckconfig_metadata;
use buck2_common::sqlite::sqlite_db::SqliteDb;
use buck2_common::sqlite::sqlite_db::SqliteIdentity;
use buck2_common::tenant::TenantKey;
use buck2_common::tenant::TenantSpec;
use buck2_core::buck2_env;
use buck2_core::cells::name::CellName;
use buck2_core::facebook_only;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::fs::project_rel_path::ProjectRelativePathBuf;
use buck2_core::is_open_source;
use buck2_core::rollout_percentage::RolloutPercentage;
use buck2_core::soft_error_or_panic;
use buck2_core::tag_result;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_events::EventSinkWithStats;
use buck2_events::daemon_id::DaemonId;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::sink::remote;
use buck2_events::sink::tee::TeeSink;
use buck2_events::source::ChannelEventSource;
use buck2_execute::blob_store::BlobStore;
use buck2_execute::dep_file_state::DepFileStore;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::execute::blocking::BlockingExecutor;
use buck2_execute::execute::blocking::BlockingExecutorFactory;
use buck2_execute::execute::blocking::IoRequest;
use buck2_execute::execute::blocking::with_lifetime_guard;
use buck2_execute::materialize::materializer::FinalArtifactMaterialization;
use buck2_execute::materialize::materializer::Materializer;
use buck2_execute::re::client::init_induced_cache_misses;
use buck2_execute::re::invocation_re_settings::invocation_re_use_case;
use buck2_execute::re::manager::ReConnectionManager;
use buck2_execute::re::ttl_refresh::ReChecker;
use buck2_execute::re::ttl_refresh::StandaloneTtlRefreshCounters;
use buck2_execute::re::ttl_refresh::TtlRefresher;
use buck2_execute_impl::executors::local::ForkserverAccess;
use buck2_execute_impl::materializers::deferred::AccessTimesUpdates;
use buck2_execute_impl::materializers::deferred::DeferredMaterializer;
use buck2_execute_impl::materializers::deferred::DeferredMaterializerConfigs;
use buck2_execute_impl::materializers::deferred::TtlRefreshConfiguration;
use buck2_execute_impl::materializers::deferred::clean_stale::CleanStaleConfig;
use buck2_execute_impl::materializers::v2::core as disk_state;
use buck2_execute_impl::materializers::v2::core::DiskStateMaterializer;
use buck2_execute_impl::re::paranoid_download::ParanoidDownloader;
use buck2_execute_impl::sqlite::dep_file_state_db::PersistedDepFileStore;
use buck2_execute_impl::sqlite::incremental_state_db::IncrementalDbState;
use buck2_execute_impl::sqlite::materializer_db::MaterializerState;
use buck2_execute_impl::sqlite::materializer_db::MaterializerStateSqliteDb;
use buck2_file_watcher::dep_files::DepFileCache;
use buck2_file_watcher::dep_files::create_dep_file_cache;
use buck2_file_watcher::file_watcher::FileWatcher;
use buck2_file_watcher::ignores::FileWatcherIgnores;
use buck2_fs::cwd::WorkingDirectory;
use buck2_fs::paths::abs_norm_path::AbsNormPath;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_hash::BuckFixedSortedMap;
use buck2_hash::BuckMutMap;
use buck2_http::HttpClient;
use buck2_http::HttpClientBuilder;
use buck2_re_configuration::RemoteExecutionStaticMetadata;
use buck2_re_configuration::RemoteExecutionStaticMetadataImpl;
use buck2_resource_control::buck_cgroup_tree::BuckCgroupTree;
use buck2_resource_control::memory_tracker;
use buck2_resource_control::memory_tracker::MemoryTrackerHandle;
use buck2_server_ctx::concurrency::ConcurrencyHandler;
use buck2_server_ctx::ctx::LockedPreviousCommandData;
use buck2_settings::DaemonBuckSettings;
use buck2_settings::PackageVisibilityDefaultIntersection;
use buck2_wrapper_common::invocation_id::TraceId;
use dice::Dice;
use dice::EvictionPolicy;
use dice_futures::cancellation::CancellationContext;
use dupe::Dupe;
use fbinit::FacebookInit;
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::future::FutureExt;
use futures::future::Shared;
use host_sharing::NamedSemaphores;
use remote::ScribeConfig;
use tokio::runtime::Handle;
use tokio::sync::Mutex;
use tokio::sync::OnceCell;
use tokio::sync::Semaphore;
use tracing::Instrument;

use crate::active_commands::ActiveCommandDropGuard;
use crate::ctx::BaseServerCommandContext;
use crate::daemon::check_working_dir;
use crate::daemon::dice::TenantDiceProvider;
use crate::daemon::disk_state::DiskStateOptions;
use crate::daemon::disk_state::delete_unknown_disk_state;
use crate::daemon::disk_state::maybe_initialize_dep_file_sqlite_db;
use crate::daemon::disk_state::maybe_initialize_incremental_sqlite_db;
use crate::daemon::disk_state::maybe_initialize_materializer_sqlite_db;
use crate::daemon::forkserver::maybe_launch_forkserver;
use crate::daemon::io_provider::create_io_provider;
use crate::daemon::panic::DaemonStatePanicDiceDump;
use crate::daemon::server::BuckdServerInitPreferences;
use crate::daemon::server::TenantDiceInputs;
use crate::daemon::server::TenantStateInitPreferences;
use crate::daemon::tenting_provider::create_tenting_acl_provider;
use crate::paging::EvictionGate;
use crate::paging::PageOutThresholds;
use crate::snapshot::DepFileDbSizeSampler;

/// For a buckd process there is a single DaemonState created at startup and never destroyed.
#[derive(Allocative)]
pub struct DaemonState {
    #[allocative(skip)]
    fb: fbinit::FacebookInit,

    /// This holds the main data shared across different commands.
    pub(crate) data: Arc<DaemonStateData>,

    /// Our working directory.
    working_directory: WorkingDirectory,
}

#[derive(Allocative)]
pub(crate) struct PersistedDepFileCache {
    #[allocative(skip)]
    pub(crate) store: Arc<dyn DepFileStore>,

    #[allocative(skip)]
    pub(crate) db_size: Arc<DepFileDbSizeSampler>,
}

/// State scoped to one logical tenant.
///
/// A tenant owns the DICE command state for one `(project root, isolation)` pair and refers to the
/// repository services used by that state. Keeping the two levels explicit allows repository state
/// to be shared independently of tenant-specific DICE state.
#[derive(Allocative)]
pub struct TenantState {
    pub repo: Arc<RepoState>,

    pub(crate) cancellation: Arc<CancellationScope>,

    /// The DICE graph and the command concurrency state for this tenant.
    pub(crate) dice_manager: Arc<ConcurrencyHandler>,

    #[allocative(skip)]
    pub(crate) eviction_gate: Arc<EvictionGate>,
    #[allocative(skip)]
    disk_ownership: Arc<Semaphore>,
}

/// Services and caches associated with a repository-backed tenant.
///
/// These are nested under [`TenantState`] even while each tenant constructs a distinct instance.
/// That ownership boundary leaves room for selected repository services to be shared later.
#[derive(Allocative)]
pub struct RepoState {
    /// Stable paths used by tenant services. Invocation cwd is not stored here.
    pub paths: TenantPaths,

    /// Synced every time we run a command.
    /// None under `--no-buckd`: the daemon serves exactly one command, so there is nothing to
    /// invalidate between commands.
    pub(crate) file_watcher: Option<Arc<dyn FileWatcher>>,

    /// Settled every time we run a command.
    pub io: Arc<dyn IoProvider>,

    /// Built from every cell's `project.ignore` when the daemon started, and not again.
    pub(crate) ignore_set: Arc<IgnoreSet>,

    /// Most materializations go through the materializer, providing a single point
    /// where the most expensive network and fs IO operations are performed. It
    /// needs access to the `ReConnectionManager` to download from RE. It must
    /// live for the entire lifetime of the daemon, in order to allow deferred
    /// materializations to work properly between distinct build commands.
    pub(crate) materializer: Arc<dyn Materializer>,

    /// Whether to consult the offline-cache buck-out dir for network action
    /// outputs prior to running them. If no cached output exists, the action
    /// (download_file, cas_artifact) will execute normally.
    ///
    /// This supports fully-offline builds, where network actions like
    /// download_file have an execution component that is inherently non-local (
    /// e.g. making a HEAD request against the remote artifact to determine if
    /// it needs to be downloaded again).
    pub use_network_action_output_cache: bool,

    /// Whether a command selecting this repo should ask the client to restart the daemon after an
    /// error.
    pub restart_daemon_on_error: bool,

    /// What buck2 state to store on disk, ex. materializer state on sqlite
    pub disk_state_options: DiskStateOptions,

    #[allocative(skip)]
    pub create_unhashed_outputs_lock: Arc<Mutex<()>>,

    /// A unique identifier for the materializer state.
    pub materializer_state_identity: Option<SqliteIdentity>,

    /// Which materializer `materializer` is, `deferred` or `v2`; reported in command metadata.
    pub materializer_implementation: &'static str,

    /// Tracks data about previous command (e.g. configs)
    pub previous_command_data: Arc<LockedPreviousCommandData>,

    /// State of the Incremental Action DB for content-based hash paths
    #[allocative(skip)]
    pub incremental_db_state: Arc<IncrementalDbState>,

    /// Persisted local dep-file cache and its size sampler for this repo, if enabled.
    pub(crate) persisted_dep_file_cache: Option<PersistedDepFileCache>,

    /// Live local dep-file cache for this repo.
    pub dep_file_cache: Arc<dyn DepFileCache>,

    /// If enabled, paranoid RE downloads.
    pub paranoid: Option<ParanoidDownloader>,

    /// The RE connection for this repo, managed such that all concurrently active build commands
    /// use the same connection. Once there are no active build commands, the connection is
    /// terminated.
    pub re_client_manager: Arc<ReConnectionManager>,

    /// Executor for blocking I/O against this repo's project root.
    pub blocking_executor: Arc<dyn BlockingExecutor>,

    pub buckconfig_metadata: BuckFixedSortedMap<String, String>,

    /// Tags to be logged per command.
    pub tags: Vec<String>,

    /// Config used to display system warnings
    pub system_warning_config: SystemWarningConfig,

    /// Whether to verify on each command that the Eden daemon backing this repo's `io` has not
    /// restarted underneath us. A restart invalidates cached state and file handles, so affected
    /// commands fail fast instead of hanging (`buck2.detect_eden_restart`).
    ///
    /// Repo-scoped because the identity baseline it checks against is captured per `io`, from the
    /// Eden socket named by this repo's checkout. Repos on one machine normally share an Eden
    /// daemon, so the answer is usually the same for all of them, but nothing requires that.
    pub detect_eden_restart: bool,

    /// Whether a finishing command schedules a background sweep of the local-action
    /// scratch dirs (`buck-out/<iso>/tmp*`) once the daemon is idle
    /// (`buck2.clean_scratch_on_idle`). The sweep runs through this repo's `materializer`.
    pub(crate) clean_scratch_on_idle: bool,

    /// Whether a write action's content stays off its output path until something needs the
    /// file (`buck2.defer_write_actions`): the deferred core's doing, at `declare_write`; the
    /// write action's under the disk-state core, which keeps the content in the BlobStore's
    /// memory. Read once, with the core's configuration, so that every command of this repo
    /// answers the same.
    pub(crate) materializer_defers_writes: bool,

    /// How often this repo's `materializer` refreshes TTLs (`buck2.ttl_refresh_frequency_seconds`).
    /// The daemon-wide walk over the digest interner runs at the first repo's.
    pub(crate) ttl_refresh_frequency: Duration,

    /// Resource-pressure thresholds for automatic idle page-out, selected for this tenant's
    /// isolation. `None` disables automatic idle page-out for this tenant.
    pub(crate) page_out_on_idle: Option<PageOutThresholds>,
}

struct TenantStateInit<'a> {
    fb: FacebookInit,
    paths: TenantPaths,
    init_ctx: &'a TenantStateInitPreferences,
    legacy_cells: &'a BuckConfigBasedCells,
    root_config: &'a LegacyBuckConfig,
    final_artifact_materialization: FinalArtifactMaterialization,
    runtime: &'a Handle,
    shared: DaemonSharedServices<'a>,
    disk_ownership: Arc<Semaphore>,
    cancellation: Arc<CancellationScope>,
    dice_provider: &'a TenantDiceProvider,
}

struct TenantInitializationGate {
    path: String,
}

impl IoRequest for TenantInitializationGate {
    fn execute(self: Box<Self>, _project_fs: &ProjectRoot) -> buck2_error::Result<()> {
        std::fs::write(format!("{}.started", self.path), [])?;
        let released = format!("{}.release", self.path);
        while !Path::new(&released).try_exists()? {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }
}

struct DaemonSharedServices<'a> {
    blocking_executor_factory: &'a BlockingExecutorFactory,
    scribe_sink: Option<&'a Arc<dyn EventSinkWithStats>>,
    memory_tracker: Option<&'a MemoryTrackerHandle>,
    daemon_id: &'a DaemonId,
}

#[derive(Allocative)]
struct TenantStateFactory {
    dice_provider: TenantDiceProvider,
    #[allocative(skip)]
    fb: FacebookInit,
    init_ctx: TenantStateInitPreferences,
    #[allocative(skip)]
    final_artifact_materialization: FinalArtifactMaterialization,
    #[allocative(skip)]
    runtime: Handle,
}

impl TenantStateFactory {
    async fn create(
        &self,
        paths: TenantPaths,
        shared: DaemonSharedServices<'_>,
        disk_ownership: Arc<Semaphore>,
        cancellation: Arc<CancellationScope>,
    ) -> buck2_error::Result<Arc<TenantState>> {
        let fs = paths.project_root().clone();
        let legacy_cells = BuckConfigBasedCells::parse_with_config_args(&fs, &[]).await?;
        let cells = &legacy_cells.cell_resolver;
        let root_config = &legacy_cells
            .parse_single_cell(cells.root_cell(), &fs)
            .await?;

        self.create_with_loaded_config(
            paths,
            &legacy_cells,
            root_config,
            shared,
            disk_ownership,
            cancellation,
        )
        .await
    }

    async fn create_with_loaded_config(
        &self,
        paths: TenantPaths,
        legacy_cells: &BuckConfigBasedCells,
        root_config: &LegacyBuckConfig,
        shared: DaemonSharedServices<'_>,
        disk_ownership: Arc<Semaphore>,
        cancellation: Arc<CancellationScope>,
    ) -> buck2_error::Result<Arc<TenantState>> {
        TenantState::create(TenantStateInit {
            fb: self.fb,
            paths,
            init_ctx: &self.init_ctx,
            legacy_cells,
            root_config,
            final_artifact_materialization: self.final_artifact_materialization,
            runtime: &self.runtime,
            shared,
            disk_ownership,
            cancellation,
            dice_provider: &self.dice_provider,
        })
        .await
    }
}

impl TenantState {
    async fn create(init: TenantStateInit<'_>) -> buck2_error::Result<Arc<Self>> {
        let TenantStateInit {
            fb,
            paths,
            init_ctx,
            legacy_cells,
            root_config,
            final_artifact_materialization,
            runtime,
            shared,
            disk_ownership,
            cancellation,
            dice_provider,
        } = init;
        let lifetime = cancellation.register()?;
        let permit = disk_ownership
            .clone()
            .acquire_owned()
            .await
            .internal_error("Tenant disk ownership gate should remain open")?;
        let process_lock = TenantDiskLock::acquire(&paths, Duration::from_secs(10)).await?;
        let disk_lease = Arc::new((permit, process_lock, lifetime));
        let buck_out_path = paths.buck_out_path();
        tokio::fs::create_dir_all(&buck_out_path)
            .await
            .tag(ErrorTag::InvalidBuckOut)
            .buck_error_context("Error creating buck_out_path")?;

        if !cancellation.observer().is_alive_sync() {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Tenant generation was evicted during initialization"
            ));
        }

        let fs = paths.project_root().clone();
        let cells = &legacy_cells.cell_resolver;

        let default_digest_algorithm =
            buck2_env!("BUCK_DEFAULT_DIGEST_ALGORITHM", type=DigestAlgorithmFamily)?;

        let default_digest_algorithm = default_digest_algorithm.unwrap_or_else(|| {
            if is_open_source() {
                DigestAlgorithmFamily::Sha256
            } else {
                DigestAlgorithmFamily::Sha1
            }
        });

        let digest_algorithms = init_ctx
            .daemon_startup_config
            .startup_options
            .digest_algorithms
            .as_ref()
            .map(|algos| {
                algos
                    .split(',')
                    .map(DigestAlgorithmFamily::from_str)
                    .collect::<Result<_, _>>()
            })
            .transpose()
            .buck_error_context("Invalid digest_algorithms")?
            .unwrap_or_else(|| vec![default_digest_algorithm])
            .into_iter()
            .map(convert_algorithm_kind)
            .collect::<Result<Vec<_>, _>>()?;

        let preferred_source_algorithm = init_ctx
            .daemon_startup_config
            .startup_options
            .source_digest_algorithm
            .as_deref()
            .map(|a| convert_algorithm_kind(a.parse()?))
            .transpose()
            .buck_error_context("Invalid source_digest_algorithm")?;

        let digest_config = DigestConfig::leak_new(digest_algorithms, preferred_source_algorithm)
            .buck_error_context("Error initializing DigestConfig")?;
        init_induced_cache_misses(digest_config.cas_digest_config())?;

        // TODO(rafaelc): merge configs from all cells once they are consistent
        let static_metadata = Arc::new(RemoteExecutionStaticMetadata::from_legacy_config(
            root_config,
        )?);

        let mut cell_configs: BuckMutMap<CellName, _> = BuckMutMap::default();
        for (cell, _) in cells.cells() {
            let config = legacy_cells.parse_single_cell(cell, &fs).await?;
            cell_configs.insert(cell, config);
        }
        let ignore_set = Arc::new(compute_ignore_set(cells.dupe(), &cell_configs)?);
        let file_watcher_ignores = FileWatcherIgnores::new(ignore_set.dupe());

        let disk_state_options = DiskStateOptions::new(root_config)?;
        let blocking_executor = with_lifetime_guard(
            shared.blocking_executor_factory.for_project(fs.dupe()),
            disk_lease,
            Some(cancellation.observer()),
        );

        if let Some(gate) = buck2_env!(
            "BUCK2_TEST_BLOCK_TENANT_INITIALIZATION",
            applicability = testing
        )? {
            let (isolation, gate) = gate.split_once(',').ok_or_else(|| {
                buck2_error!(
                    ErrorTag::Input,
                    "Expected isolation,path for the tenant initialization gate"
                )
            })?;
            if paths.isolation().as_str() == isolation {
                blocking_executor
                    .execute_io(
                        Box::new(TenantInitializationGate {
                            path: gate.to_owned(),
                        }),
                        CancellationContext::never_cancelled(),
                    )
                    .await?;
            }
        }

        if !cancellation.observer().is_alive_sync() {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Tenant generation was evicted during initialization"
            ));
        }
        let cache_dir_path = paths.cache_dir_path();
        let valid_cache_dirs = paths.valid_cache_dirs();

        let disk_state_core = init_ctx
            .daemon_startup_config
            .startup_options
            .materializer_v2_version
            .is_some_and(|wanted| disk_state::ROLLOUT_VERSION >= wanted);
        let deferred_materializer_configs = {
            let defer_write_actions = root_config
                .parse::<RolloutPercentage>(BuckconfigKeyRef {
                    section: "buck2",
                    property: "defer_write_actions",
                })?
                .unwrap_or_else(RolloutPercentage::never)
                .roll();

            // RE will refresh any TTL < 1 hour, so we check twice an hour and refresh any TTL
            // < 1 hour.
            let ttl_refresh_frequency = root_config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "ttl_refresh_frequency_seconds",
                })?
                .unwrap_or(1800);

            let ttl_refresh_min_ttl = root_config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "ttl_refresh_min_ttl_seconds",
                })?
                .unwrap_or(3600);

            let ttl_refresh_enabled = root_config
                .parse::<RolloutPercentage>(BuckconfigKeyRef {
                    section: "buck2",
                    property: "ttl_refresh_enabled",
                })?
                .unwrap_or_else(RolloutPercentage::never)
                .roll();

            let update_access_times =
                AccessTimesUpdates::try_new_from_config_value(root_config.get(BuckconfigKeyRef {
                    section: "buck2",
                    property: "update_access_times",
                }))?;

            let verbose_materializer_log = root_config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "verbose_materializer_event_log",
                })?
                .unwrap_or(false);

            let strict_leases = root_config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "materializer_strict_leases",
                })?
                .unwrap_or(false);

            let mut clean_stale_config = CleanStaleConfig::from_buck_config(root_config)?;
            // The disk-state core's TTLs are refreshed by the daemon-wide walk whatever the knob
            // says of the deferred core's own refresher.
            clean_stale_config
                .suppress_unmaterialize_without_ttl_refresh(ttl_refresh_enabled || disk_state_core);
            let readonly_outputs = root_config
                .parse::<RolloutPercentage>(BuckconfigKeyRef {
                    section: "buck2",
                    property: "readonly_outputs",
                })?
                .unwrap_or_else(RolloutPercentage::never)
                .roll();

            DeferredMaterializerConfigs {
                materialize_final_artifacts: matches!(
                    final_artifact_materialization,
                    FinalArtifactMaterialization::Enabled
                ),
                defer_write_actions,
                ttl_refresh: TtlRefreshConfiguration {
                    frequency: Duration::from_secs(ttl_refresh_frequency),
                    min_ttl: jiff::SignedDuration::from_secs(ttl_refresh_min_ttl),
                    enabled: ttl_refresh_enabled,
                },
                update_access_times,
                verbose_materializer_log,
                clean_stale_config,
                readonly_outputs,
                strict_leases,
            }
        };
        let materializer_defers_writes = deferred_materializer_configs.defer_write_actions;
        let ttl_refresh_frequency = deferred_materializer_configs.ttl_refresh.frequency;

        let use_eden_thrift_read = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "use_eden_thrift_read",
            })?
            .unwrap_or(cfg!(any(target_os = "macos", target_os = "windows")));

        tracing::info!("Creating materializer...");
        let (io, _, (materializer_db, materializer_state), incremental_db_state, dep_file_db) =
            futures::future::try_join5(
                create_io_provider(
                    fb,
                    fs.dupe(),
                    root_config,
                    digest_config.cas_digest_config(),
                    init_ctx.enable_trace_io,
                    use_eden_thrift_read,
                ),
                (blocking_executor.dupe() as Arc<dyn BlockingExecutor>).execute_io_inline(|| {
                    // Using `execute_io_inline` is just out of convenience.
                    // It doesn't really matter what's used here since there's no IO-heavy
                    // operations on daemon startup.
                    delete_unknown_disk_state(&cache_dir_path, &valid_cache_dirs)
                }),
                maybe_initialize_materializer_sqlite_db(
                    &disk_state_options,
                    paths.clone(),
                    blocking_executor.dupe() as Arc<dyn BlockingExecutor>,
                    root_config,
                    &deferred_materializer_configs,
                    digest_config,
                    init_ctx,
                    shared.daemon_id,
                ),
                maybe_initialize_incremental_sqlite_db(
                    paths.clone(),
                    blocking_executor.dupe() as Arc<dyn BlockingExecutor>,
                    root_config,
                    shared.daemon_id,
                ),
                maybe_initialize_dep_file_sqlite_db(
                    &disk_state_options,
                    paths.clone(),
                    blocking_executor.dupe() as Arc<dyn BlockingExecutor>,
                    root_config,
                    shared.daemon_id,
                ),
            )
            .await?;

        // The cache is opt-in and best-effort, so a store that cannot be built leaves the
        // repo running without persistence rather than failing startup.
        let persisted_dep_file_cache = dep_file_db.and_then(|dep_file_db| {
            match PersistedDepFileStore::try_new(dep_file_db, digest_config) {
                Ok(store) => {
                    let store = Arc::new(store) as Arc<dyn DepFileStore>;
                    Some(PersistedDepFileCache {
                        db_size: DepFileDbSizeSampler::start(store.dupe(), runtime),
                        store,
                    })
                }
                Err(e) => {
                    soft_error_or_panic!(
                        "dep_file_store_init",
                        buck2_error::buck2_error!(
                            buck2_error::ErrorTag::Tier0,
                            "Failed to start the persisted dep-file cache; continuing without \
                             it. {}",
                            e
                        )
                    );
                    None
                }
            }
        });
        let incremental_db_state = Arc::new(incremental_db_state);
        let materializer_state_identity = materializer_db.as_ref().map(|d| d.identity().clone());

        let re_client_manager = Arc::new(ReConnectionManager::new(
            fb,
            false,
            10,
            static_metadata.dupe(),
            Some(paths.re_logs_dir()),
            paths.buck_out_path(),
            init_ctx.daemon_startup_config.startup_options.paranoid,
        ));
        // Used only to dispatch events to scribe that are not associated with a specific command
        // (ex. materializer clean up events).
        let daemon_dispatcher = if let Some(sink) = shared.scribe_sink {
            EventDispatcher::new(
                TraceId::null(),
                shared.daemon_id.dupe(),
                sink.dupe().to_event_sync(),
            )
        } else {
            EventDispatcher::null()
        };
        let stash = io
            .project_root()
            .resolve(paths.buck_out_dir())
            .join(ForwardRelativePath::unchecked_new("stash"));
        if disk_state_core {
            // The daemon has one stash, started empty. It lives under this repo's buck-out until
            // it has a home of the daemon's own; a second repo in the process would share it.
            BlobStore::global().attach_stash(
                stash,
                digest_config.cas_digest_config(),
                blocking_executor.dupe(),
            );
        } else {
            // Nothing refers to what a previous daemon left, whichever core it ran.
            buck2_execute::blob_store::discard_stash(&stash, blocking_executor.dupe());
        }
        let materializer = RepoState::create_materializer(
            disk_state_core,
            io.project_root().dupe(),
            digest_config,
            paths.buck_out_dir(),
            re_client_manager.dupe(),
            blocking_executor.dupe(),
            deferred_materializer_configs,
            materializer_db,
            materializer_state,
            daemon_dispatcher,
        )?;
        tracing::info!("Creating tenting ACL provider...");
        let tenting_acl_provider = create_tenting_acl_provider(fb, paths.project_root());

        tracing::info!("Constructing DICE...");
        let (dice_manager, _root, eviction_gate) = init_ctx
            .construct_dice(TenantDiceInputs {
                io: io.dupe(),
                digest_config,
                ignore_set: ignore_set.dupe(),
                root_config,
                tenting_acl_provider,
                dice_state_path: paths.dice_state_path().as_ref(),
                provider: dice_provider,
                lifetime_guard: blocking_executor.lifetime_guard(),
                memory_pressure: shared
                    .memory_tracker
                    .map(|tracker| tracker.memory_pressure()),
            })
            .await?;

        let dep_file_cache = create_dep_file_cache();

        let file_watcher = if init_ctx.in_process
            && root_config
                .get(BuckconfigKeyRef {
                    section: "buck2",
                    property: "file_watcher",
                })
                .is_none()
        {
            None
        } else {
            tracing::info!("Creating file watcher...");
            Some(
                <dyn FileWatcher>::new(
                    fb,
                    paths.project_root(),
                    root_config,
                    file_watcher_ignores,
                    dep_file_cache.dupe(),
                )
                .with_buck_error_context(|| {
                    format!(
                        "Error creating a FileWatcher for project root `{}`",
                        paths.project_root()
                    )
                })?,
            )
        };

        // TODO(bobyf): Eagerly sync the file watcher here once the DICE commit panic is fixed.

        let use_network_action_output_cache = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "use_network_action_output_cache",
            })?
            .unwrap_or(false);

        let detect_eden_restart = root_config
            .parse(BuckconfigKeyRef {
                section: "buck2",
                property: "detect_eden_restart",
            })?
            .unwrap_or(false);

        let paranoid = if init_ctx.daemon_startup_config.startup_options.paranoid {
            Some(ParanoidDownloader::new(
                fs.clone(),
                blocking_executor.dupe(),
                re_client_manager.dupe(),
                paths.paranoid_cache_dir(),
            ))
        } else {
            None
        };

        let remote_dep_files_enabled = root_config
            .parse(BuckconfigKeyRef {
                section: "build",
                property: "remote_dep_file_cache_enabled",
            })?
            .unwrap_or(false);

        let action_freezing_enabled = init_ctx
            .daemon_startup_config
            .startup_options
            .resource_control
            .enable_suspension;

        let evicts_on_memory_pressure = init_ctx
            .daemon_startup_config
            .startup_options
            .hydration
            .as_ref()
            .is_some_and(|hydration| hydration.eviction_policy == EvictionPolicy::OnMemoryPressure);
        // Under `on_memory_pressure` only the memory tracker evicts, so without one an idle
        // page-out would write rows for values that never leave memory.
        let evictions_impossible = evicts_on_memory_pressure && shared.memory_tracker.is_none();
        let page_out_on_idle = init_ctx
            .daemon_startup_config
            .idle_page_out_config_for_isolation_dir(paths.isolation())
            .filter(|_| !evictions_impossible)
            .map(|hydration| PageOutThresholds {
                min_free_disk_gb: hydration.page_out_min_free_disk_gb,
            });
        if evictions_impossible {
            tracing::info!(
                "Idle page-out disabled: `hydration.eviction_policy` is `on_memory_pressure` \
                 and this daemon has no memory tracker to see pressure with"
            );
        }

        let tags = vec![
            format!(
                "dice-sharing:{}",
                init_ctx.daemon_startup_config.startup_options.share_dice
            ),
            // TODO(scottcao): Delete this tag since now hash all commands is always enabled.
            "hash-all-commands:true".to_owned(),
            format!(
                "sqlite-materializer-state:{}",
                disk_state_options.sqlite_materializer_state
            ),
            format!("paranoid:{}", paranoid.is_some()),
            format!("remote-dep-files:{}", remote_dep_files_enabled),
            #[cfg(fbcode_build)]
            format!(
                "respect-file-symlinks:{}",
                static_metadata.respect_file_symlinks
            ),
            "disable-eager-write-dispatch-v2:true".to_owned(),
            format!("use-eden-thrift-read:{}", use_eden_thrift_read),
            format!("memory_tracker-enabled:{}", shared.memory_tracker.is_some()),
            format!("action-freezing-enabled:{}", action_freezing_enabled),
            format!("has-cgroup:{}", shared.memory_tracker.is_some()),
        ];

        let repo = Arc::new(RepoState {
            paths,
            file_watcher,
            io,
            ignore_set,
            materializer,
            use_network_action_output_cache,
            restart_daemon_on_error: root_config
                .parse::<RolloutPercentage>(BuckconfigKeyRef {
                    section: "buck2",
                    property: "restarter",
                })?
                .unwrap_or_else(RolloutPercentage::never)
                .roll(),
            disk_state_options,
            create_unhashed_outputs_lock: Arc::new(Mutex::new(())),
            materializer_state_identity,
            materializer_implementation: if disk_state_core { "v2" } else { "deferred" },
            previous_command_data: LockedPreviousCommandData::new(),
            incremental_db_state,
            persisted_dep_file_cache,
            dep_file_cache,
            paranoid,
            re_client_manager,
            blocking_executor,
            buckconfig_metadata: parse_buckconfig_metadata(root_config),
            tags,
            system_warning_config: SystemWarningConfig::from_config(root_config)?,
            detect_eden_restart,
            materializer_defers_writes,
            ttl_refresh_frequency,
            clean_scratch_on_idle: root_config
                .parse::<RolloutPercentage>(BuckconfigKeyRef {
                    section: "buck2",
                    property: "clean_scratch_on_idle",
                })?
                .unwrap_or_else(RolloutPercentage::never)
                .roll(),
            page_out_on_idle,
        });

        #[cfg(fbcode_build)]
        {
            let root_path = std::path::PathBuf::from(repo.paths.project_root().root().as_os_str());
            if !buck2_env!("BUCK2_DISABLE_EDEN_HEALTH_CHECK", bool)?
                && detect_eden::is_eden(root_path).unwrap_or(false)
            {
                tracing::trace!("EdenFS root detected; starting health check job");
                crate::daemon::server::eden_health::edenfs_health_check(
                    fb,
                    repo.paths.project_root().dupe(),
                )
                .await;
            }
        }

        Ok(Arc::new(Self {
            repo,
            dice_manager,
            eviction_gate,
            disk_ownership,
            cancellation,
        }))
    }

    pub async fn spawn_dice_dump(
        &self,
        path: &Path,
        format: DiceDumpFormat,
    ) -> buck2_error::Result<()> {
        crate::daemon::dice_dump::dice_dump_spawn(self.dice_manager.unsafe_dice(), path, format)
            .await
    }
}

impl RepoState {
    fn create_materializer(
        disk_state_core: bool,
        fs: ProjectRoot,
        digest_config: DigestConfig,
        buck_out_path: ProjectRelativePathBuf,
        re_client_manager: Arc<ReConnectionManager>,
        blocking_executor: Arc<dyn BlockingExecutor>,
        deferred_materializer_configs: DeferredMaterializerConfigs,
        materializer_db: Option<MaterializerStateSqliteDb>,
        materializer_state: Option<MaterializerState>,
        daemon_dispatcher: EventDispatcher,
    ) -> buck2_error::Result<Arc<dyn Materializer>> {
        if disk_state_core {
            Ok(Arc::new(DiskStateMaterializer::new(
                fs,
                digest_config,
                buck_out_path,
                re_client_manager,
                blocking_executor,
                deferred_materializer_configs,
                materializer_db,
                materializer_state,
                daemon_dispatcher,
            )?))
        } else {
            Ok(Arc::new(DeferredMaterializer::new(
                fs,
                digest_config,
                buck_out_path,
                re_client_manager,
                blocking_executor,
                deferred_materializer_configs,
                materializer_db,
                materializer_state,
                daemon_dispatcher,
            )?))
        }
    }
}

/// Tenant states known to this daemon.
///
/// Clients without an explicit identity may select the sole initialized tenant. Daemon status
/// keeps the initial tenant's metadata without retaining its disk services.
#[derive(Allocative)]
struct TenantStateRegistry {
    /// Prefer the initial tenant for snapshots while it remains alive.
    #[allocative(skip)]
    initial_state: Weak<TenantState>,
    initial_metadata: TenantStatusMetadata,
    #[allocative(skip)]
    disk_owners: StdMutex<BuckMutMap<TenantKey, Weak<Semaphore>>>,
    #[allocative(skip)]
    dice_engines: StdMutex<Vec<Weak<Dice>>>,
    tenants: StdMutex<BuckMutMap<TenantKey, Arc<TenantStateSlot<TenantState>>>>,
    #[allocative(skip)]
    next_generation: AtomicU64,
    #[allocative(skip)]
    draining: AtomicBool,
}

struct TenantStateSlot<T: Allocative> {
    spec: TenantSpec,
    generation: u64,
    state: OnceCell<Arc<T>>,
    disk_ownership: Arc<Semaphore>,
    cancellation: Arc<CancellationScope>,
}

impl<T: Allocative> Allocative for TenantStateSlot<T> {
    fn visit<'a, 'b: 'a>(&self, visitor: &'a mut allocative::Visitor<'b>) {
        let mut visitor = visitor.enter_self_sized::<Self>();
        visitor.visit_field(allocative::Key::new("spec"), &self.spec);
        visitor.visit_field(allocative::Key::new("generation"), &self.generation);
        if let Some(state) = self.state.get() {
            visitor.visit_field(allocative::Key::new("state"), state);
        }
        visitor.exit();
    }
}

impl<T: Allocative> TenantStateSlot<T> {
    fn new(
        spec: TenantSpec,
        generation: u64,
        disk_ownership: Arc<Semaphore>,
        cancellation: Arc<CancellationScope>,
    ) -> Self {
        Self {
            spec,
            generation,
            state: OnceCell::new(),
            disk_ownership,
            cancellation,
        }
    }

    async fn get_or_try_init<F, Fut>(&self, init: F) -> buck2_error::Result<Arc<T>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = buck2_error::Result<Arc<T>>>,
    {
        let _initialization = self.cancellation.register()?;
        self.state.get_or_try_init(init).await.map(Arc::clone)
    }

    fn is_initialized(&self) -> bool {
        self.state.get().is_some()
    }
}

impl TenantStateRegistry {
    async fn new(initial_tenant: Arc<TenantState>) -> buck2_error::Result<Self> {
        let spec = TenantSpec::from_tenant_paths(&initial_tenant.repo.paths);
        let disk_ownership = initial_tenant.disk_ownership.clone();
        let entry = Arc::new(TenantStateSlot::new(
            spec.clone(),
            1,
            disk_ownership.clone(),
            initial_tenant.cancellation.clone(),
        ));
        entry
            .state
            .set(initial_tenant.clone())
            .map_err(|_| buck2_error!(ErrorTag::Tier0, "Initial tenant slot should be empty"))?;
        Ok(Self {
            initial_state: Arc::downgrade(&initial_tenant),
            initial_metadata: TenantStatusMetadata::new(&initial_tenant.repo),
            tenants: StdMutex::new(BuckMutMap::from_iter([(spec.key().clone(), entry)])),
            disk_owners: StdMutex::new(BuckMutMap::from_iter([(
                spec.key().clone(),
                Arc::downgrade(&disk_ownership),
            )])),
            dice_engines: StdMutex::new(vec![Arc::downgrade(
                initial_tenant.dice_manager.unsafe_dice(),
            )]),
            next_generation: AtomicU64::new(2),
            draining: AtomicBool::new(false),
        })
    }

    async fn get_or_create<F, Fut>(
        &self,
        spec: TenantSpec,
        create: F,
    ) -> buck2_error::Result<Arc<TenantState>>
    where
        F: FnOnce(Arc<Semaphore>, Arc<CancellationScope>) -> Fut + Send + 'static,
        Fut: Future<Output = buck2_error::Result<Arc<TenantState>>> + Send + 'static,
    {
        let key = spec.key().clone();
        let requested_spec = spec.clone();
        let entry = {
            let mut tenants = self
                .tenants
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if self.draining.load(Ordering::Acquire) {
                return Err(buck2_error!(
                    ErrorTag::Input,
                    "The daemon is shutting down after its final tenant was evicted"
                ));
            }
            tenants
                .entry(key.clone())
                .or_insert_with(|| {
                    let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
                    let mut owners = self
                        .disk_owners
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    owners.retain(|_, gate| gate.strong_count() != 0);
                    let disk_ownership =
                        owners.get(&key).and_then(Weak::upgrade).unwrap_or_else(|| {
                            let gate = Arc::new(Semaphore::new(1));
                            owners.insert(key.clone(), Arc::downgrade(&gate));
                            gate
                        });
                    Arc::new(TenantStateSlot::new(
                        spec,
                        generation,
                        disk_ownership,
                        Arc::new(CancellationScope::new()),
                    ))
                })
                .clone()
        };
        if entry.spec != requested_spec {
            return Err(buck2_error!(
                buck2_error::ErrorTag::Input,
                "Tenant key `{:?}` was requested with conflicting specifications",
                key
            ));
        }

        let state = entry
            .get_or_try_init(|| async {
                let disk_ownership = entry.disk_ownership.clone();
                let cancellation = entry.cancellation.clone();
                let (initialize, _) =
                    entry
                        .cancellation
                        .spawn(&Handle::current(), move |context| {
                            context
                                .critical_section(move || create(disk_ownership, cancellation))
                                .boxed()
                        });
                let state = initialize.await.map_err(|error| {
                    buck2_error!(
                        ErrorTag::Input,
                        "Tenant generation was evicted during initialization: {error}"
                    )
                })??;
                let actual_key = TenantKey::from_tenant_paths(&state.repo.paths);
                if actual_key != key {
                    return Err(buck2_error!(
                        buck2_error::ErrorTag::Input,
                        "Constructed tenant key `{:?}` did not match requested key `{:?}`",
                        actual_key,
                        key
                    ));
                }
                let mut engines = self
                    .dice_engines
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                engines.retain(|engine| engine.strong_count() != 0);
                engines.push(Arc::downgrade(state.dice_manager.unsafe_dice()));
                Ok(state)
            })
            .await?;
        let tenants = self
            .tenants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !tenants
            .get(&key)
            .is_some_and(|registered| Arc::ptr_eq(registered, &entry))
        {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Tenant generation was evicted during initialization"
            ));
        }
        Ok(state)
    }

    fn status_tenant(&self) -> Option<Arc<TenantState>> {
        self.initial_state.upgrade().or_else(|| {
            self.tenants
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .find_map(|entry| entry.state.get().cloned())
        })
    }

    fn legacy_tenant(&self) -> Option<Arc<TenantState>> {
        let tenants = self
            .tenants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let initialized_tenant_count = tenants
            .values()
            .filter(|entry| entry.is_initialized())
            .count();
        (initialized_tenant_count == 1)
            .then(|| {
                tenants
                    .values()
                    .find_map(|entry| entry.state.get().map(Arc::clone))
            })
            .flatten()
    }

    fn evict(&self, key: &TenantKey) -> Option<TenantEviction> {
        let mut tenants = self
            .tenants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let entry = tenants.remove(key)?;
        entry.cancellation.cancel();
        if let Some(state) = entry.state.get() {
            state.dice_manager.retire();
        }
        let final_tenant = tenants.is_empty();
        if final_tenant {
            self.draining.store(true, Ordering::Release);
        }
        Some(TenantEviction {
            generation: entry.generation,
            final_tenant,
            completed: Self::finish_eviction(vec![entry]),
        })
    }

    fn evict_project(
        &self,
        project_root: &AbsNormPath,
        isolation: Option<&FileName>,
    ) -> ProjectTenantEviction {
        let mut tenants = self
            .tenants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut generations = Vec::new();
        let mut evicted = Vec::new();
        tenants.retain(|key, entry| {
            let matches = key.project_root() == project_root
                && isolation.is_none_or(|isolation| key.isolation() == isolation);
            if matches {
                entry.cancellation.cancel();
                evicted.push(entry.clone());
                if let Some(state) = entry.state.get() {
                    state.dice_manager.retire();
                }
                generations.push(entry.generation);
            }
            !matches
        });
        generations.sort_unstable();
        let final_tenant = !generations.is_empty() && tenants.is_empty();
        if final_tenant {
            self.draining.store(true, Ordering::Release);
        }
        ProjectTenantEviction {
            generations,
            final_tenant,
            completed: Self::finish_eviction(evicted),
        }
    }

    fn finish_eviction(
        entries: Vec<Arc<TenantStateSlot<TenantState>>>,
    ) -> Shared<BoxFuture<'static, ()>> {
        let scopes = entries
            .iter()
            .map(|entry| entry.cancellation.clone())
            .collect::<Vec<_>>();
        drop(entries);
        let cleanup = tokio::spawn(async move {
            futures::stream::iter(scopes)
                .map(|scope| async move { scope.wait().await })
                .buffer_unordered(16)
                .collect::<Vec<_>>()
                .await;
        });
        cleanup
            .map(|result| result.expect("Tenant cancellation cleanup should finish"))
            .boxed()
            .shared()
    }

    fn list(&self) -> Vec<TenantStateInfo> {
        let tenants = self
            .tenants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut result: Vec<_> = tenants
            .values()
            .map(|entry| TenantStateInfo {
                key: entry.spec.key().clone(),
                generation: entry.generation,
                initialized: entry.is_initialized(),
            })
            .collect();
        result.sort_by(|a, b| {
            a.key
                .project_root()
                .as_os_str()
                .cmp(b.key.project_root().as_os_str())
                .then_with(|| a.key.isolation().as_str().cmp(b.key.isolation().as_str()))
        });
        result
    }
}

#[derive(Allocative)]
pub(crate) struct TenantStatusMetadata {
    pub paths: TenantPaths,
    pub io_provider: String,
    pub trace_io_enabled: bool,
    pub materializer_state_identity: Option<SqliteIdentity>,
}

impl TenantStatusMetadata {
    fn new(repo: &RepoState) -> Self {
        Self {
            paths: repo.paths.clone(),
            io_provider: repo.io.name().to_owned(),
            trace_io_enabled: TracingIoProvider::from_io(&*repo.io).is_some(),
            materializer_state_identity: repo.materializer_state_identity.clone(),
        }
    }
}

pub struct TenantStateInfo {
    pub key: TenantKey,
    pub generation: u64,
    pub initialized: bool,
}

pub struct TenantEviction {
    pub generation: u64,
    pub final_tenant: bool,
    pub(crate) completed: Shared<BoxFuture<'static, ()>>,
}

pub struct ProjectTenantEviction {
    pub generations: Vec<u64>,
    pub final_tenant: bool,
    pub(crate) completed: Shared<BoxFuture<'static, ()>>,
}

fn tenant_paths_from_client_context(
    client_context: &ClientContext,
) -> buck2_error::Result<Option<TenantPaths>> {
    let Some(identity) = &client_context.tenant_identity else {
        return Ok(None);
    };

    let project_root = AbsNormPathBuf::try_from(identity.project_root.clone())
        .buck_error_context("Invalid tenant project root in client context")?;
    let isolation = FileNameBuf::try_from(identity.isolation.clone())
        .buck_error_context("Invalid tenant isolation in client context")?;
    Ok(Some(TenantPaths::new(
        ProjectRoot::new_unchecked(project_root),
        isolation,
    )))
}

/// DaemonStateData is the main shared data across all commands and repos. It's lazily initialized
/// on the first command that requires it.
#[derive(Allocative)]
pub struct DaemonStateData {
    tenants: TenantStateRegistry,
    tenant_state_factory: TenantStateFactory,

    /// Daemon-wide scheduling resources for repo-scoped blocking executors.
    pub blocking_executor_factory: Arc<BlockingExecutorFactory>,

    pub(crate) forkserver: ForkserverAccess,

    #[allocative(skip)]
    pub scribe_sink: Option<Arc<dyn EventSinkWithStats>>,

    pub start_time: Instant,

    /// Http client used for materializer and RunAction implementations.
    pub http_client: HttpClient,

    /// Spawner
    pub spawner: Arc<BuckSpawner>,

    /// Tracks memory usage. Used to make scheduling decisions.
    #[allocative(skip)]
    pub memory_tracker: Option<MemoryTrackerHandle>,

    /// A unique identifier for this instance of the daemon
    pub daemon_id: DaemonId,

    /// Cgroup path of the process that launched this daemon before Buck moved the daemon into its
    /// managed cgroup.
    pub daemon_originating_cgroup: Option<String>,

    /// Semaphores for running actions locally. These need to be shared across commands.
    #[allocative(skip)]
    pub named_semaphores_for_run_actions: Arc<NamedSemaphores>,

    /// Effective `package_visibility.default_intersection`, fixed for the daemon's
    /// lifetime (sourced from the daemon's Buck Settings).
    pub(crate) package_visibility_default_intersection: PackageVisibilityDefaultIntersection,

    /// Counters of the TTL refresher that walks the file digest interner. The refresher itself
    /// is a task on the daemon's runtime and lives as long as it does.
    pub(crate) standalone_ttl_refresh: Arc<StandaloneTtlRefreshCounters>,
}

// SAFETY: `_assert_send_sync` requires every field to be `Send + Sync`. Spelled out
// so that auto-trait proofs for command futures stop here; see `dice::DiceComputations`.
unsafe impl Send for DaemonStateData {}
unsafe impl Sync for DaemonStateData {}

fn _assert_send_sync(
    DaemonStateData {
        tenants,
        tenant_state_factory,
        blocking_executor_factory,
        forkserver,
        scribe_sink,
        start_time,
        http_client,
        spawner,
        memory_tracker,
        daemon_id,
        daemon_originating_cgroup,
        named_semaphores_for_run_actions,
        package_visibility_default_intersection,
        standalone_ttl_refresh,
    }: &DaemonStateData,
) {
    fn assert<T: Send + Sync>(_: &T) {}
    assert(tenants);
    assert(tenant_state_factory);
    assert(blocking_executor_factory);
    assert(forkserver);
    assert(scribe_sink);
    assert(start_time);
    assert(http_client);
    assert(spawner);
    assert(memory_tracker);
    assert(daemon_id);
    assert(daemon_originating_cgroup);
    assert(named_semaphores_for_run_actions);
    assert(package_visibility_default_intersection);
    assert(standalone_ttl_refresh);
}

impl DaemonStateData {
    pub(crate) fn daemon_buck_settings(&self) -> &DaemonBuckSettings {
        self.tenant_state_factory
            .init_ctx
            .daemon_startup_config
            .daemon_buck_settings()
    }

    fn repo_shared_services(&self) -> DaemonSharedServices<'_> {
        DaemonSharedServices {
            blocking_executor_factory: &self.blocking_executor_factory,
            scribe_sink: self.scribe_sink.as_ref(),
            memory_tracker: self.memory_tracker.as_ref(),
            daemon_id: &self.daemon_id,
        }
    }

    /// Select or initialize the tenant addressed by a client command.
    pub async fn tenant_for_client_context(
        self: &Arc<Self>,
        client_context: &ClientContext,
    ) -> buck2_error::Result<Arc<TenantState>> {
        let Some(paths) = tenant_paths_from_client_context(client_context)? else {
            return self.tenant_for_legacy_client();
        };
        let spec = TenantSpec::from_tenant_paths(&paths);
        let data = self.dupe();

        self.tenants
            .get_or_create(spec, move |disk_ownership, cancellation| async move {
                data.tenant_state_factory
                    .create(
                        paths,
                        data.repo_shared_services(),
                        disk_ownership,
                        cancellation,
                    )
                    .await
            })
            .await
    }

    /// Select a tenant for an RPC added before requests carried a client context.
    pub async fn tenant_for_optional_client_context(
        self: &Arc<Self>,
        client_context: Option<&ClientContext>,
    ) -> buck2_error::Result<Arc<TenantState>> {
        match client_context {
            Some(client_context) => self.tenant_for_client_context(client_context).await,
            None => self.tenant_for_legacy_client(),
        }
    }

    fn tenant_for_legacy_client(&self) -> buck2_error::Result<Arc<TenantState>> {
        self.tenants.legacy_tenant().ok_or_else(|| {
            buck2_error!(
                ErrorTag::Input,
                "Client did not provide a tenant identity after the daemon began serving multiple tenants"
            )
        })
    }

    /// A live tenant for daemon snapshots that do not carry a tenant identity.
    pub fn status_tenant(&self) -> Option<Arc<TenantState>> {
        self.tenants.status_tenant()
    }

    pub(crate) fn initial_tenant_metadata(&self) -> &TenantStatusMetadata {
        &self.tenants.initial_metadata
    }

    pub(crate) async fn wait_for_dice_idle(&self) {
        let engines = self
            .tenants
            .dice_engines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        futures::stream::iter(engines)
            .map(|dice| async move { dice.wait_for_idle().await })
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
    }

    pub fn evict_tenant_for_client_context(
        &self,
        client_context: &ClientContext,
    ) -> buck2_error::Result<Option<TenantEviction>> {
        let paths = tenant_paths_from_client_context(client_context)?.ok_or_else(|| {
            buck2_error!(
                ErrorTag::Input,
                "Tenant eviction requires an explicit tenant identity"
            )
        })?;
        Ok(self.tenants.evict(&TenantKey::from_tenant_paths(&paths)))
    }

    pub fn evict_project_tenants_for_client_context(
        &self,
        client_context: &ClientContext,
        isolation: Option<FileNameBuf>,
    ) -> buck2_error::Result<ProjectTenantEviction> {
        let paths = tenant_paths_from_client_context(client_context)?.ok_or_else(|| {
            buck2_error!(
                ErrorTag::Input,
                "Repository tenant eviction requires an explicit tenant identity"
            )
        })?;
        Ok(self
            .tenants
            .evict_project(paths.project_root().root(), isolation.as_deref()))
    }

    pub fn tenants(&self) -> Vec<TenantStateInfo> {
        self.tenants.list()
    }

    pub fn dice_dump(&self, path: &Path, format: DiceDumpFormat) -> buck2_error::Result<()> {
        crate::daemon::dice_dump::dice_dump(
            self.status_tenant()
                .internal_error("No live tenant is available for DICE dump")?
                .dice_manager
                .unsafe_dice(),
            path,
            format,
        )
    }
}

impl DaemonStatePanicDiceDump for DaemonStateData {
    fn dice_dump(&self, path: &Path, format: DiceDumpFormat) -> buck2_error::Result<()> {
        self.dice_dump(path, format)
    }
}

impl DaemonState {
    pub(crate) async fn new(
        fb: fbinit::FacebookInit,
        paths: InvocationPaths,
        init_ctx: BuckdServerInitPreferences,
        rt: &Handle,
        final_artifact_materialization: FinalArtifactMaterialization,
        working_directory: WorkingDirectory,
        cgroup_tree: Option<BuckCgroupTree>,
        daemon_id: DaemonId,
    ) -> Result<Self, buck2_error::Error> {
        let data = Self::init_data(
            fb,
            paths,
            init_ctx,
            rt,
            final_artifact_materialization,
            cgroup_tree,
            daemon_id,
        )
        .await
        .map_err(|e| {
            e.context("Error initializing DaemonStateData")
                .tag([ErrorTag::DaemonStateInitFailed])
        })?;

        crate::daemon::panic::initialize(data.dupe());

        tracing::info!("Daemon state is ready.");

        let state = DaemonState {
            fb,
            data,
            working_directory,
        };
        Ok(state)
    }

    // Creates the initial DaemonStateData.
    // Starts up the watchman query.
    async fn init_data(
        fb: fbinit::FacebookInit,
        paths: InvocationPaths,
        init_ctx: BuckdServerInitPreferences,
        rt: &Handle,
        final_artifact_materialization: FinalArtifactMaterialization,
        cgroup_tree: Option<BuckCgroupTree>,
        daemon_id: DaemonId,
    ) -> buck2_error::Result<Arc<DaemonStateData>> {
        if buck2_env!(
            "BUCK2_TEST_INIT_DAEMON_ERROR",
            bool,
            applicability = testing
        )? {
            // TODO(minglunli): Errors here don't actually make it to invocation records which should be fixed
            return Err(buck2_error::buck2_error!(
                ErrorTag::TestOnly,
                "Injected init daemon error"
            ));
        }

        let daemon_state_data_rt = rt.clone();
        // Owned, because tenant construction happens in the spawned initialization future.
        let tenant_state_rt = rt.clone();
        let init_fut = async move {
            let invocation_paths = paths;
            let paths = invocation_paths.tenant_paths();
            let fs = paths.project_root().clone();

            tracing::info!("Reading config...");
            let legacy_cells = BuckConfigBasedCells::parse_with_config_args(&fs, &[]).await?;

            tracing::info!("Starting...");

            let cells = &legacy_cells.cell_resolver;
            let root_config = &legacy_cells
                .parse_single_cell(cells.root_cell(), &fs)
                .await?;

            let buffer_size = root_config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "event_log_buffer_size",
                })?
                .unwrap_or(10000);
            let retry_backoff = Duration::from_millis(
                root_config
                    .parse(BuckconfigKeyRef {
                        section: "buck2",
                        property: "event_log_retry_backoff_duration_ms",
                    })?
                    .unwrap_or(500),
            );
            let retry_attempts = root_config
                .parse(BuckconfigKeyRef {
                    section: "buck2",
                    property: "event_log_retry_attempts",
                })?
                .unwrap_or(5);
            let message_batch_size = root_config.parse(BuckconfigKeyRef {
                section: "buck2",
                property: "event_log_message_batch_size",
            })?;
            tracing::info!("Initializing scribe sink...");
            let scribe_sink = Self::init_scribe_sink(
                fb,
                ScribeConfig {
                    buffer_size,
                    retry_backoff,
                    retry_attempts,
                    message_batch_size,
                    thrift_timeout: Duration::from_secs(1),
                },
            )
            .buck_error_context("failed to init scribe sink")?;

            let blocking_executor_factory = Arc::new(BlockingExecutorFactory::create()?);

            let http_client = http_client_from_startup_config(&init_ctx.daemon_startup_config)
                .await
                .buck_error_context("Error creating HTTP client")?
                .build();

            tracing::info!("Creating memory tracker...");
            let memory_tracker = memory_tracker::create_memory_tracker(
                cgroup_tree,
                &init_ctx
                    .daemon_startup_config
                    .startup_options
                    .resource_control,
                &daemon_id,
            )
            .await?;

            // The forkserver creates its state directory recursively, so it does not depend on
            // the materializer creating buck-out first. One daemon-global forkserver serves every
            // repo; requests carry an absolute cwd.
            tracing::info!("Launching forkserver...");
            let forkserver = maybe_launch_forkserver(
                root_config,
                &invocation_paths.forkserver_state_dir(&daemon_id.to_string())?,
                memory_tracker.as_ref().map(|m| &m.cgroup_tree),
                &invocation_paths.isolation,
            )
            .await?;

            let (init_ctx, daemon_originating_cgroup) = init_ctx.split();
            let tenant_state_factory = TenantStateFactory {
                dice_provider: TenantDiceProvider::new(
                    init_ctx.daemon_startup_config.startup_options.share_dice,
                    invocation_paths
                        .shared_dice_state_path(&daemon_id.to_string())?
                        .to_path_buf(),
                ),
                fb,
                init_ctx,
                final_artifact_materialization,
                runtime: tenant_state_rt,
            };
            let tenant = tenant_state_factory
                .create_with_loaded_config(
                    paths,
                    &legacy_cells,
                    root_config,
                    DaemonSharedServices {
                        blocking_executor_factory: &blocking_executor_factory,
                        scribe_sink: scribe_sink.as_ref(),
                        memory_tracker: memory_tracker.as_ref(),
                        daemon_id: &daemon_id,
                    },
                    Arc::new(Semaphore::new(1)),
                    Arc::new(CancellationScope::new()),
                )
                .await?;

            // One walk over the process-wide digest interner, on the first tenant's RE
            // connection and cadence: the BlobStore's, which refreshes CAS TTLs and drops what
            // nothing holds.
            let standalone_ttl_refresh = BlobStore::global().start_refresh(
                TtlRefresher::new(Arc::new(ReChecker::new(
                    tenant.repo.re_client_manager.dupe(),
                    invocation_re_use_case(root_config)?,
                ))),
                tenant.repo.ttl_refresh_frequency,
            );

            let package_visibility_default_intersection = tenant_state_factory
                .init_ctx
                .daemon_startup_config
                .daemon_buck_settings()
                .package_visibility
                .default_intersection();
            let tenants = TenantStateRegistry::new(tenant).await?;
            Ok(Arc::new(DaemonStateData {
                tenants,
                tenant_state_factory,
                blocking_executor_factory,
                forkserver,
                scribe_sink,
                start_time: std::time::Instant::now(),
                http_client,
                spawner: Arc::new(BuckSpawner::new(daemon_state_data_rt)),
                memory_tracker,
                daemon_id: daemon_id.dupe(),
                daemon_originating_cgroup,
                named_semaphores_for_run_actions: Arc::new(NamedSemaphores::new()),
                package_visibility_default_intersection,
                standalone_ttl_refresh,
            }))
        };
        let daemon_listener_span = tracing::Span::current();
        rt.spawn(init_fut.instrument(daemon_listener_span)).await?
    }

    fn init_scribe_sink(
        fb: FacebookInit,
        config: ScribeConfig,
    ) -> buck2_error::Result<Option<Arc<dyn EventSinkWithStats>>> {
        facebook_only();
        remote::new_remote_event_sink_if_enabled(fb, config)
            .map(|maybe_scribe| maybe_scribe.map(|scribe| Arc::new(scribe) as _))
    }

    /// Prepares an event stream for a request by bootstrapping an event source and EventDispatcher pair. The given
    /// EventDispatcher will log to the returned EventSource and (optionally) to Scribe if enabled via buckconfig.
    pub async fn prepare_events(
        &self,
        trace_id: TraceId,
    ) -> buck2_error::Result<(ChannelEventSource, EventDispatcher)> {
        // facebook only: logging events to Scribe.
        facebook_only();
        let (events, sink) = buck2_events::create_source_sink_pair();
        let data = self.data();
        let dispatcher = if let Some(scribe_sink) = data.scribe_sink.dupe() {
            EventDispatcher::new(
                trace_id,
                self.data.daemon_id.dupe(),
                TeeSink::new(scribe_sink.to_event_sync(), sink),
            )
        } else {
            EventDispatcher::new(trace_id, self.data.daemon_id.dupe(), sink)
        };
        Ok((events, dispatcher))
    }

    /// Prepares a ServerCommandContext for processing a complex command (that accesses the dice computation graph, for example).
    ///
    /// This initializes (if necessary) the shared daemon state and syncs the watchman query (to flush any recent filesystem events).
    pub async fn prepare_command(
        &self,
        tenant: Arc<TenantState>,
        dispatcher: EventDispatcher,
        drop_guard: ActiveCommandDropGuard,
    ) -> buck2_error::Result<BaseServerCommandContext> {
        let data = self.data();
        let repo = &tenant.repo;

        dispatcher.instant_event(buck2_data::RestartConfiguration {
            enable_restarter: repo.restart_daemon_on_error,
        });

        tag_result!(
            "eden_not_connected",
            check_working_dir::check_working_dir(),
            daemon_in_memory_state_is_corrupted: true,
            task: false
        )?;

        self.validate_cwd()
            .buck_error_context("Error validating working directory")?;

        self.validate_buck_out_mount(&repo.paths)
            .buck_error_context("Error validating buck-out mount")?;

        dispatcher.instant_event(buck2_data::TagEvent {
            tags: repo.tags.clone(),
        });

        // Sync any FS changes and invalidate DICE state if necessary.  Get the Eden
        // version of the underlying system in parallel if available, and fail fast if the
        // Eden daemon restarted underneath us (which leaves cached state and file handles
        // stale and would otherwise surface as a silent hang).
        let verify_eden_identity = async {
            if repo.detect_eden_restart {
                repo.io.verify_eden_identity().await
            } else {
                Ok(())
            }
        };
        let (_, eden_version, ()) = futures::future::try_join3(
            repo.io.settle(),
            repo.io.eden_version(),
            verify_eden_identity,
        )
        .await?;

        dispatcher.instant_event(buck2_data::IoProviderInfo { eden_version });

        Ok(BaseServerCommandContext {
            _fb: self.fb,
            events: dispatcher,
            tenant,
            daemon: data.dupe(),
            _drop_guard: drop_guard,
        })
    }

    pub fn data(&self) -> Arc<DaemonStateData> {
        self.data.dupe()
    }

    pub fn validate_cwd(&self) -> buck2_error::Result<()> {
        let res = self.working_directory.is_stale().and_then(|stale| {
            if stale {
                Err(buck2_error!(
                    buck2_error::ErrorTag::DaemonStaleWorkingDir,
                    "Buck appears to be running in a stale working directory. \
                     This will likely lead to failed or slow builds. \
                     To remediate, restart Buck2."
                ))
            } else {
                Ok(())
            }
        });

        tag_result!(
            "stale_cwd",
            res,
            daemon_in_memory_state_is_corrupted: true,
            task: false
        )?;

        Ok(())
    }

    pub fn validate_buck_out_mount(&self, paths: &TenantPaths) -> buck2_error::Result<()> {
        #[cfg(fbcode_build)]
        {
            use buck2_core::warn_user;
            use buck2_fs::error::IoResultExt;
            use buck2_fs::fs_util;

            let project_root = paths.project_root().root();
            if !detect_eden::is_eden(project_root.to_path_buf())? {
                return Ok(());
            }

            let buck_out_root = project_root.join(TenantPaths::buck_out_dir_prefix());

            if let Some(buck_out_root_meta) = fs_util::symlink_metadata_if_exists(buck_out_root)? {
                // If buck-out is a symlink, we'll be happy with that.
                if buck_out_root_meta.is_symlink() {
                    return Ok(());
                }

                // If we are on UNIX, then buck-out could also be on a different device from the repo.
                // We don't check which kind of device, we just assume it's not mounted completely
                // wrong.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;

                    let project_device = fs_util::symlink_metadata(project_root)
                        .categorize_internal()?
                        .dev();
                    let buck_out_device = buck_out_root_meta.dev();

                    if project_device != buck_out_device {
                        return Ok(());
                    }
                }
            }

            warn_user!(
                "eden_buck_out",
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Environment,
                    "Buck is running in an Eden repository, but `buck-out` is not redirected. \
                     This will likely lead to failed or slow builds. \
                     To remediate, run `eden redirect fixup`."
                )
            );
        }
        #[cfg(not(fbcode_build))]
        {
            let _paths = paths;
        }

        Ok(())
    }
}

fn convert_algorithm_kind(kind: DigestAlgorithmFamily) -> buck2_error::Result<DigestAlgorithm> {
    buck2_error::Ok(match kind {
        DigestAlgorithmFamily::Sha1 => DigestAlgorithm::Sha1,
        DigestAlgorithmFamily::Sha256 => DigestAlgorithm::Sha256,
        DigestAlgorithmFamily::Blake3 => DigestAlgorithm::Blake3,
        DigestAlgorithmFamily::Blake3Keyed => {
            #[cfg(fbcode_build)]
            {
                DigestAlgorithm::Blake3Keyed
            }

            #[cfg(not(fbcode_build))]
            {
                // We probably should just add it as a separate buckconfig, there is
                // zero reason not to.
                return Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "{} is not supported in the open source build",
                    kind
                ));
            }
        }
    })
}

/// Sensible defaults for http client when building from a DaemonStartupConfig.
const DEFAULT_MAX_REDIRECTS: usize = 10;
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5000;
const DEFAULT_READ_TIMEOUT_MS: u64 = 10000;

/// Customize an http client based on http.* legacy buckconfigs.
async fn http_client_from_startup_config(
    config: &DaemonStartupConfig,
) -> buck2_error::Result<HttpClientBuilder> {
    let http = &config.startup_options.http;
    let mut builder = if is_open_source() {
        HttpClientBuilder::oss().await?
    } else {
        HttpClientBuilder::internal().await?
    };
    #[cfg(fbcode_build)]
    builder
        .with_internal_proxy_from_env(&http.proxy_env_allowlist)
        .await?;
    builder.with_max_redirects(http.max_redirects.unwrap_or(DEFAULT_MAX_REDIRECTS));
    builder.with_http2(http.http2);
    builder.with_max_concurrent_requests(http.max_concurrent_requests);

    match http.connect_timeout() {
        Timeout::Value(d) => {
            builder.with_connect_timeout(Some(d));
        }
        Timeout::Default => {
            builder.with_connect_timeout(Some(Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS)));
        }
        _ => {}
    }
    match http.read_timeout() {
        Timeout::Value(d) => {
            builder.with_read_timeout(Some(d));
        }
        Timeout::Default => {
            builder.with_read_timeout(Some(Duration::from_millis(DEFAULT_READ_TIMEOUT_MS)));
        }
        _ => {}
    }
    match http.write_timeout() {
        Timeout::Value(d) => {
            builder.with_write_timeout(Some(d));
        }
        Timeout::Default | Timeout::NoTimeout => {}
    }

    Ok(builder)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use buck2_cli_proto::TenantIdentity;
    use buck2_common::legacy_configs::configs::testing::parse;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::file_name::FileNameBuf;
    use buck2_settings::DaemonBuckSettings;
    use indoc::indoc;

    use super::*;

    fn tenant_spec() -> TenantSpec {
        let project_root = if cfg!(windows) {
            "C:\\project"
        } else {
            "/project"
        };
        TenantSpec::from_tenant_paths(&TenantPaths::new(
            ProjectRoot::new_unchecked(
                AbsNormPathBuf::try_from(project_root.to_owned())
                    .expect("test project root should be absolute and normalized"),
            ),
            FileNameBuf::try_from("v2".to_owned()).expect("test isolation should be a file name"),
        ))
    }

    #[tokio::test]
    async fn tenant_entry_initializes_once() -> buck2_error::Result<()> {
        let entry = TenantStateSlot::<usize>::new(
            tenant_spec(),
            1,
            Arc::new(Semaphore::new(1)),
            Arc::new(CancellationScope::new()),
        );
        let init_count = AtomicUsize::new(0);
        let first = entry.get_or_try_init(|| async {
            init_count.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
            Ok(Arc::new(1))
        });
        let second = entry.get_or_try_init(|| async {
            init_count.fetch_add(1, Ordering::Relaxed);
            Ok(Arc::new(2))
        });

        let (first, second) = tokio::join!(first, second);
        let first = first?;
        let second = second?;
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(init_count.load(Ordering::Relaxed), 1);
        Ok(())
    }

    #[tokio::test]
    async fn tenant_entry_retries_failed_initialization() -> buck2_error::Result<()> {
        let entry = TenantStateSlot::<usize>::new(
            tenant_spec(),
            1,
            Arc::new(Semaphore::new(1)),
            Arc::new(CancellationScope::new()),
        );
        let first = entry
            .get_or_try_init(|| async {
                Err(buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "injected initialization failure"
                ))
            })
            .await;
        assert!(first.is_err());
        assert!(!entry.is_initialized());

        let state = entry.get_or_try_init(|| async { Ok(Arc::new(1)) }).await?;
        assert_eq!(*state, 1);
        assert!(entry.is_initialized());
        Ok(())
    }

    #[test]
    fn tenant_paths_from_client_identity() -> buck2_error::Result<()> {
        let project_root = if cfg!(windows) {
            "C:\\project"
        } else {
            "/project"
        };
        let client_context = ClientContext {
            tenant_identity: Some(TenantIdentity {
                project_root: project_root.to_owned(),
                isolation: "v2".to_owned(),
            }),
            ..Default::default()
        };

        let paths = tenant_paths_from_client_context(&client_context)?
            .expect("the client supplied a tenant identity");
        assert_eq!(paths.project_root().to_string(), project_root);
        assert_eq!(paths.isolation().as_str(), "v2");
        Ok(())
    }

    #[test]
    fn missing_client_identity_uses_compatibility_fallback() -> buck2_error::Result<()> {
        assert!(tenant_paths_from_client_context(&ClientContext::default())?.is_none());
        Ok(())
    }

    #[test]
    fn invalid_client_project_root_is_rejected() {
        let client_context = ClientContext {
            tenant_identity: Some(TenantIdentity {
                project_root: "relative".to_owned(),
                isolation: "v2".to_owned(),
            }),
            ..Default::default()
        };

        assert!(tenant_paths_from_client_context(&client_context).is_err());
    }

    #[test]
    fn invalid_client_isolation_is_rejected() {
        let project_root = if cfg!(windows) {
            "C:\\project"
        } else {
            "/project"
        };
        let client_context = ClientContext {
            tenant_identity: Some(TenantIdentity {
                project_root: project_root.to_owned(),
                isolation: "nested/isolation".to_owned(),
            }),
            ..Default::default()
        };

        assert!(tenant_paths_from_client_context(&client_context).is_err());
    }

    #[tokio::test]
    async fn test_from_startup_config_defaults_internal() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let builder =
            http_client_from_startup_config(&DaemonStartupConfig::testing_empty()).await?;
        assert_eq!(DEFAULT_MAX_REDIRECTS, builder.max_redirects().unwrap());
        assert_eq!(
            builder.supports_vpnless(),
            buck2_certs::certs::supports_vpnless()
        );
        assert_eq!(
            Some(Duration::from_millis(DEFAULT_CONNECT_TIMEOUT_MS)),
            builder.connect_timeout()
        );
        assert_eq!(
            Some(Duration::from_millis(DEFAULT_READ_TIMEOUT_MS)),
            builder.read_timeout()
        );
        assert_eq!(None, builder.write_timeout());

        Ok(())
    }

    #[tokio::test]
    async fn test_from_startup_config_overrides() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let config = parse(
            &[(
                "config",
                indoc!(
                    r#"
                    [http]
                    max_redirects = 5
                    connect_timeout_ms = 10
                    write_timeout_ms = 5
                    "#
                ),
            )],
            "config",
        )?;
        let startup_config =
            DaemonStartupConfig::new(&config, &DaemonBuckSettings::default(), false)?;
        let builder = http_client_from_startup_config(&startup_config).await?;
        assert_eq!(5, builder.max_redirects().unwrap());
        assert_eq!(Some(Duration::from_millis(10)), builder.connect_timeout());
        assert_eq!(
            Some(Duration::from_millis(DEFAULT_READ_TIMEOUT_MS)),
            builder.read_timeout()
        );
        assert_eq!(Some(Duration::from_millis(5)), builder.write_timeout());

        Ok(())
    }

    #[tokio::test]
    async fn test_from_startup_config_zero_for_unset() -> buck2_error::Result<()> {
        buck2_certs::certs::maybe_setup_cryptography();
        let config = parse(
            &[(
                "config",
                indoc!(
                    r#"
                    [http]
                    connect_timeout_ms = 0
                    "#,
                ),
            )],
            "config",
        )?;
        let startup_config =
            DaemonStartupConfig::new(&config, &DaemonBuckSettings::default(), false)?;
        let builder = http_client_from_startup_config(&startup_config).await?;
        assert_eq!(None, builder.connect_timeout());
        assert_eq!(
            Some(Duration::from_millis(DEFAULT_READ_TIMEOUT_MS)),
            builder.read_timeout()
        );
        assert_eq!(None, builder.write_timeout());

        Ok(())
    }
}
