/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::future;
use std::io;
use std::io::Write;
use std::marker::PhantomData;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use allocative::Allocative;
use async_trait::async_trait;
use buck2_build_api::configure_dice::DiceEngineConfiguration;
use buck2_build_api::configure_dice::DiceEngineConfigurationInputs;
use buck2_build_api::configure_dice::DiceRootConfiguration;
use buck2_build_api::configure_dice::configure_dice_data_for_buck;
use buck2_certs::validate::CertState;
use buck2_certs::validate::CertStateReport;
use buck2_certs::validate::INVALID_CERTS_TAG;
use buck2_certs::validate::check_cert_state_for;
use buck2_certs::validate::validate_certs;
use buck2_cli_proto::daemon_api_server::*;
use buck2_cli_proto::*;
use buck2_common::buckd_connection::BUCK_AUTH_TOKEN_HEADER;
use buck2_common::cancellation::CancellationScope;
use buck2_common::ignores::ignore_set::IgnoreSet;
use buck2_common::init::DaemonStartupConfig;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_common::io::IoProvider;
use buck2_common::legacy_configs::configs::LegacyBuckConfig;
use buck2_common::memory;
use buck2_common::sqlite::sqlite_db::SqliteIdentity;
use buck2_common::tenting::TentingAclProvider;
use buck2_core::buck2_env;
use buck2_core::error::SoftErrorContext;
use buck2_core::fs::project::ProjectRoot;
use buck2_core::logging::LogConfigurationReloadHandle;
use buck2_core::pattern::unparsed::UnparsedPatternPredicate;
use buck2_error::BuckErrorContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_events::DrainReport;
use buck2_events::DrainScope;
use buck2_events::Event;
use buck2_events::EventSinkWithStats;
use buck2_events::StreamDrainOutcome;
use buck2_events::daemon_id::DaemonId;
use buck2_events::dispatch::EventDispatcher;
use buck2_events::source::ChannelEventSource;
use buck2_execute::digest_config::DigestConfig;
use buck2_execute::materialize::materializer::FinalArtifactMaterialization;
use buck2_execute_impl::executors::local::ForkserverAccess;
use buck2_fs::async_fs_util::spawn_blocking;
use buck2_fs::cwd::WorkingDirectory;
use buck2_fs::fs_util;
use buck2_fs::fs_util::DiskSpaceStats;
use buck2_fs::fs_util::disk_space_stats;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::paths::file_name::FileNameBuf;
use buck2_interpreter::starlark_profiler::config::StarlarkProfilerConfiguration;
use buck2_profile::proto_to_profile_mode;
use buck2_profile::starlark_profiler_configuration_from_request;
use buck2_resource_control::buck_cgroup_tree::BuckCgroupTree;
use buck2_resource_control::buck_cgroup_tree::PreppedBuckCgroups;
use buck2_server_ctx::bxl::BXL_SERVER_COMMANDS;
use buck2_server_ctx::concurrency::ConcurrencyHandler;
use buck2_server_ctx::ctx::ServerCommandContextTrait;
use buck2_server_ctx::late_bindings::AUDIT_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::LSP_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::OTHER_SERVER_COMMANDS;
use buck2_server_ctx::late_bindings::QUERY_SERVER_COMMANDS;
use buck2_server_ctx::late_bindings::STARLARK_SERVER_COMMAND;
use buck2_server_ctx::late_bindings::TARGETS_SERVER_COMMANDS;
use buck2_server_ctx::partial_result_dispatcher::NoPartialResult;
use buck2_server_ctx::partial_result_dispatcher::PartialResultDispatcher;
use buck2_server_ctx::streaming_request_handler::StreamingRequestHandler;
use buck2_server_ctx::test_command::TEST_COMMAND;
use buck2_server_starlark_debug::run::run_dap_server_command;
use buck2_test::executor_launcher::get_all_test_executors;
use buck2_util::system_stats::container_memory_limit_bytes;
use buck2_util::system_stats::num_cores;
use buck2_util::system_stats::system_memory_stats;
use buck2_util::threads::thread_spawn;
use buck2_wrapper_common::invocation_id::TraceId;
use dice::BranchId;
use dice::Dice;
use dice::EvictionPolicy;
use dice_futures::cancellation::CancellationContext;
use dice_futures::cancellation::DropcancelHandle;
use dice_futures::drop::DropTogether;
use dupe::Dupe;
use futures::Future;
use futures::FutureExt;
use futures::Stream;
use futures::StreamExt;
use futures::TryFutureExt;
use futures::channel::mpsc;
use futures::channel::mpsc::UnboundedReceiver;
use futures::channel::mpsc::UnboundedSender;
use futures::future::BoxFuture;
use futures::stream;
use pagable::DeferredReadFailures;
use rand::Rng as _;
use rand::SeedableRng;
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::time::timeout;
use tokio::time::timeout_at;
use tonic::Code;
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tonic::service::Interceptor;
use tonic::service::InterceptorLayer;

use crate::active_commands::ActiveCommand;
use crate::active_commands::ActiveCommandStateWriter;
use crate::clean_stale::clean_stale_command;
use crate::ctx::ServerCommandContext;
use crate::daemon::crash::crash;
use crate::daemon::dice::TenantDiceProvider;
use crate::daemon::multi_event_stream::MultiEventStream;
use crate::daemon::server_allocative::spawn_allocative;
use crate::daemon::state::DaemonState;
use crate::file_status::file_status_command;
use crate::hydration::hydration_command;
use crate::new_generic::new_generic_command;
use crate::paging::EvictionGate;
use crate::paging::cancel_active_page_out;
use crate::profile::profile_command;
use crate::profiling_manager::StarlarkProfilingManager;
use crate::snapshot;
use crate::snapshot::SnapshotCollector;
use crate::subscription::run_subscription_server_command;
use crate::trace_io::trace_io_command;
use crate::version_control_revision;

// TODO(cjhopman): Figure out a reasonable value for this.
static DEFAULT_KILL_TIMEOUT: Duration = Duration::from_millis(500);

/// Extra time past `shutdown_deadline` before the watchdog kills the process.
///
/// Only reached if a client still holds a connection open, since `shutdown_deadline` already
/// covers the drain and the `dice.wait_for_idle()` after it. Just the return and the exit logs
/// can still be outstanding, so this is small - and it adds directly to how long `buck2 kill`
/// takes when the drain cannot finish.
static SHUTDOWN_WATCHDOG_GRACE: Duration = Duration::from_secs(1);

static DEFAULT_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(4 * 86400);
static CLEAN_STALE_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Clone)]
struct DaemonInactivityConfig {
    initial_timeout: Duration,
    standard_timeout: Duration,
    use_standard_timeout: Arc<AtomicBool>,
}

#[derive(Clone, Copy)]
enum DaemonActivity {
    CleanStale,
    Other,
}

impl DaemonInactivityConfig {
    fn try_new(
        daemon_idle_timeout_s: Option<u64>,
        started_for_clean_stale: bool,
    ) -> buck2_error::Result<Self> {
        let mut standard_timeout = daemon_idle_timeout_s
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_INACTIVITY_TIMEOUT);
        if buck2_env!(
            "BUCK2_TESTING_INACTIVITY_TIMEOUT",
            bool,
            applicability = testing
        )? {
            standard_timeout = Duration::from_secs(1);
        }

        Ok(Self {
            initial_timeout: if started_for_clean_stale {
                CLEAN_STALE_INACTIVITY_TIMEOUT.min(standard_timeout)
            } else {
                standard_timeout
            },
            standard_timeout,
            use_standard_timeout: Arc::new(AtomicBool::new(!started_for_clean_stale)),
        })
    }

    fn note_activity(&self, activity: DaemonActivity) {
        if let DaemonActivity::Other = activity {
            self.use_standard_timeout.store(true, Ordering::Release);
        }
    }

    fn current_timeout(&self) -> Duration {
        if self.use_standard_timeout.load(Ordering::Acquire) {
            self.standard_timeout
        } else {
            self.initial_timeout
        }
    }
}

#[derive(Allocative)]
struct DaemonShutdown {
    /// This channel is used to trigger a graceful shutdown of the grpc server. After
    /// an item is sent on this channel, the server will start rejecting new requests
    /// and once current requests are finished the server will shutdown. The item is
    /// the deadline for completing the graceful shutdown.
    #[allocative(skip)]
    shutdown_channel: UnboundedSender<tokio::time::Instant>,
}

impl DaemonShutdown {
    /// Trigger a graceful server shutdown with a timeout. Once the timeout expires,
    /// `spawn_shutdown_watchdog` takes the process down whether or not the drain finished.
    ///
    /// As we might be processing a `kill()` (or other) request, we cannot wait for the server to actually
    /// shutdown (as it will wait for current requests to finish), so this returns immediately.
    fn start_shutdown(&self, reason: buck2_data::DaemonShutdown, timeout: Option<Duration>) {
        // The daemon's stderr is `buckd.stderr`, which the client tails into the console and the
        // invocation record and `rage` collects: it is where a daemon's last words are looked
        // for, so the reason goes there directly.
        eprintln!("triggered shutdown: {}", reason.reason);
        crate::active_commands::broadcast_shutdown(&reason);

        let timeout = timeout.unwrap_or(DEFAULT_KILL_TIMEOUT);
        let shutdown_deadline = tokio::time::Instant::now() + timeout;

        // Ignore errors on shutdown_channel as that would mean we've already started shutdown;
        let _ = self.shutdown_channel.unbounded_send(shutdown_deadline);
    }
}

#[derive(Allocative)]
pub struct BuckdServerInitPreferences {
    pub enable_trace_io: bool,
    pub reject_materializer_state: Option<SqliteIdentity>,
    pub daemon_startup_config: DaemonStartupConfig,
    pub daemon_originating_cgroup: Option<String>,
    pub started_for_clean_stale: bool,
}

#[derive(Allocative)]
pub(crate) struct TenantStateInitPreferences {
    pub(crate) enable_trace_io: bool,
    pub(crate) reject_materializer_state: Option<SqliteIdentity>,
    pub(crate) daemon_startup_config: DaemonStartupConfig,
}

impl BuckdServerInitPreferences {
    pub(crate) fn split(self) -> (TenantStateInitPreferences, Option<String>) {
        let Self {
            enable_trace_io,
            reject_materializer_state,
            daemon_startup_config,
            daemon_originating_cgroup,
            ..
        } = self;
        (
            TenantStateInitPreferences {
                enable_trace_io,
                reject_materializer_state,
                daemon_startup_config,
            },
            daemon_originating_cgroup,
        )
    }
}

pub(crate) struct TenantDiceInputs<'a> {
    pub(crate) io: Arc<dyn IoProvider>,
    pub(crate) digest_config: DigestConfig,
    pub(crate) ignore_set: Arc<IgnoreSet>,
    pub(crate) root_config: &'a LegacyBuckConfig,
    pub(crate) tenting_acl_provider: Option<Arc<dyn TentingAclProvider>>,
    pub(crate) dice_state_path: &'a Path,
    pub(crate) provider: &'a TenantDiceProvider,
    pub(crate) lifetime_guard: Option<Arc<dyn Send + Sync>>,
    pub(crate) memory_pressure: Option<watch::Receiver<bool>>,
}

impl TenantStateInitPreferences {
    pub(crate) async fn construct_dice(
        &self,
        inputs: TenantDiceInputs<'_>,
    ) -> buck2_error::Result<(Arc<ConcurrencyHandler>, BranchId, Arc<EvictionGate>)> {
        let TenantDiceInputs {
            io,
            digest_config,
            ignore_set,
            root_config,
            tenting_acl_provider,
            dice_state_path,
            provider,
            lifetime_guard,
            memory_pressure,
        } = inputs;
        let hydration = self
            .daemon_startup_config
            .startup_options
            .hydration
            .as_ref();
        let configuration = DiceEngineConfiguration::resolve(DiceEngineConfigurationInputs {
            dice_state_path: hydration.map(|_| provider.state_path(dice_state_path)),
            pagable_storage_backend: hydration
                .map_or_else(Default::default, |h| h.pagable_storage_backend),
            page_out_policy: hydration.map_or_else(Default::default, |h| h.page_out_policy),
            eviction_policy: hydration.map_or_else(Default::default, |h| h.eviction_policy),
            defer_field_reads: self
                .daemon_startup_config
                .daemon_buck_settings()
                .hydration
                .defer_field_reads(),
        })?;
        let data = configure_dice_data_for_buck(DiceRootConfiguration {
            io,
            digest_config,
            ignore_set,
            root_config: Some(root_config),
            tenting_acl_provider,
            buck_settings: self.daemon_startup_config.daemon_buck_settings().dupe(),
        })?;
        provider
            .create_with_lifetime_guard(
                configuration,
                data,
                lifetime_guard,
                memory_pressure.filter(|_| {
                    hydration.is_some_and(|h| h.eviction_policy == EvictionPolicy::OnMemoryPressure)
                }),
            )
            .await
    }
}

#[derive(Clone)]
struct BuckCheckAuthTokenInterceptor {
    auth_token: String,
}

impl Interceptor for BuckCheckAuthTokenInterceptor {
    fn call(&mut self, request: Request<()>) -> Result<Request<()>, Status> {
        let token = match request.metadata().get(BUCK_AUTH_TOKEN_HEADER) {
            Some(token) => token,
            None => return Err(Status::unauthenticated("missing auth token")),
        };
        if !constant_time_eq::constant_time_eq(token.as_bytes(), self.auth_token.as_bytes()) {
            return Err(Status::unauthenticated("invalid auth token"));
        }

        if buck2_env!("BUCK2_TEST_FAIL_BUCKD_AUTH", bool, applicability = testing).unwrap() {
            return Err(Status::unauthenticated("injected auth error"));
        }

        Ok(request)
    }
}

#[derive(Allocative)]
pub(crate) struct BuckdServerData {
    /// The flag that is set to true when server is shutting down.
    stop_accepting_requests: AtomicBool,
    #[allocative(skip)]
    process_info: DaemonProcessInfo,
    base_daemon_constraints: buck2_cli_proto::DaemonConstraints,
    start_time: prost_types::Timestamp,
    start_instant: Instant,
    daemon_shutdown: DaemonShutdown,
    daemon_state: Arc<DaemonState>,
    #[allocative(skip)]
    cert_state: CertState,
    #[allocative(skip)]
    inactivity_config: DaemonInactivityConfig,
    #[allocative(skip)]
    command_channel: UnboundedSender<()>,
    #[allocative(skip)]
    log_reload_handle: Arc<dyn LogConfigurationReloadHandle>,
    #[allocative(skip)]
    rt: Handle,
}

/// The BuckdServer implements the DaemonApi.
///
/// Simple endpoints are implemented here and complex things will be implemented in a sibling
/// module taking just a ServerCommandContext.
#[derive(Allocative)]
pub struct BuckdServer(Arc<BuckdServerData>);

impl BuckdServer {
    fn shutdown_after_final_tenant_eviction(&self) {
        self.0
            .stop_accepting_requests
            .store(true, Ordering::Release);
        self.0.daemon_shutdown.start_shutdown(
            buck2_data::DaemonShutdown {
                reason: "The final tenant was evicted".to_owned(),
                callers: Vec::new(),
            },
            None,
        );
    }

    #[tracing::instrument(name = "daemon_listener", skip_all)]
    pub async fn run(
        fb: fbinit::FacebookInit,
        log_reload_handle: Arc<dyn LogConfigurationReloadHandle>,
        paths: InvocationPaths,
        init_ctx: BuckdServerInitPreferences,
        process_info: DaemonProcessInfo,
        in_process: bool,
        prepped_cgroups: Option<PreppedBuckCgroups>,
        base_daemon_constraints: buck2_cli_proto::DaemonConstraints,
        listener: Pin<Box<dyn Stream<Item = Result<tokio::net::TcpStream, io::Error>> + Send>>,
        rt: Handle,
        daemon_id: DaemonId,
        ready: Box<dyn FnOnce() + Send>,
    ) -> buck2_error::Result<()> {
        let now = SystemTime::now();
        let now = now.duration_since(SystemTime::UNIX_EPOCH)?;

        let (shutdown_channel, shutdown_receiver): (UnboundedSender<tokio::time::Instant>, _) =
            mpsc::unbounded();
        let (command_channel, command_receiver): (UnboundedSender<()>, _) = mpsc::unbounded();

        let final_artifact_materialization =
            FinalArtifactMaterialization::try_new_from_config_value(
                init_ctx
                    .daemon_startup_config
                    .startup_options
                    .materializations
                    .as_deref(),
            )?;

        let working_dir = paths.daemon_working_dir()?;
        fs_util::create_dir_all(&working_dir)
            .tag(ErrorTag::InvalidBuckOut)
            .buck_error_context("Error creating daemon working directory")?;

        let cwd = {
            let dir = WorkingDirectory::open(working_dir)?;
            dir.chdir_and_promise_it_will_not_change()?;
            dir
        };

        let cgroup_tree = if let Some(prepped_cgroups) = prepped_cgroups {
            Some(
                BuckCgroupTree::set_up(
                    prepped_cgroups,
                    &init_ctx
                        .daemon_startup_config
                        .startup_options
                        .resource_control,
                )
                .await?,
            )
        } else {
            None
        };

        let cert_state = CertState::new().await;
        certs_validation_background_job(cert_state.dupe()).await;

        let inactivity_config = DaemonInactivityConfig::try_new(
            init_ctx
                .daemon_startup_config
                .startup_options
                .daemon_idle_timeout_s,
            init_ctx.started_for_clean_stale,
        )?;

        let daemon_state = Arc::new(
            DaemonState::new(
                fb,
                paths,
                init_ctx,
                &rt,
                final_artifact_materialization,
                cwd,
                cgroup_tree,
                daemon_id,
            )
            .await?,
        );

        let auth_token = process_info.auth_token.clone();
        // Held past the move of `daemon_state` below, for the shutdown sequence.
        let scribe_sink = daemon_state.data().scribe_sink.dupe();
        let api_server = BuckdServer(Arc::new(BuckdServerData {
            stop_accepting_requests: AtomicBool::new(false),
            process_info,
            base_daemon_constraints,
            start_time: prost_types::Timestamp {
                seconds: now.as_secs() as i64,
                nanos: now.subsec_nanos() as i32,
            },
            start_instant: Instant::now(),
            daemon_shutdown: DaemonShutdown { shutdown_channel },
            daemon_state: daemon_state.clone(),
            cert_state,
            inactivity_config: inactivity_config.clone(),
            command_channel,
            log_reload_handle,
            rt,
        }));

        let (shutdown, shutdown_deadline) = server_shutdown_signal(
            command_receiver,
            shutdown_receiver,
            inactivity_config,
            in_process,
        );
        let server = buck2_grpc::server_builder()
            .layer(InterceptorLayer::new(BuckCheckAuthTokenInterceptor {
                auth_token,
            }))
            .add_service(
                DaemonApiServer::new(api_server)
                    .max_encoding_message_size(usize::MAX)
                    .max_decoding_message_size(usize::MAX),
            )
            .serve_with_incoming_shutdown(listener, shutdown);

        tracing::info!("Starting server");
        if let Some(sleep_secs) = buck2_env!(
            "BUCK2_TEST_INIT_DATA_SLEEP_SECS",
            type = u64,
            applicability = testing
        )? {
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
        }

        // The listener accepted connections since before the daemon state existed, but answers
        // them only from here on.
        ready();
        let server_result = server.await;

        let shutdown_deadline = shutdown_deadline
            .await
            .unwrap_or_else(|_| tokio::time::Instant::now() + DEFAULT_KILL_TIMEOUT);
        if timeout_at(shutdown_deadline, daemon_state.data().wait_for_dice_idle())
            .await
            .is_err()
        {
            eprintln!("timed out waiting for DICE tasks to finish during shutdown");
        }

        // Finish the Build Event Service streams before the process exits. The sink's worker
        // would do it when the sink is dropped, but that is after this function returns, and
        // the process does not wait for it; the server then shows the build running forever.
        if let Some(sink) = scribe_sink {
            match timeout(Duration::from_secs(10), sink.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("remote event sink streams not finished cleanly: {:#}", e)
                }
                Err(_) => {
                    tracing::warn!("timed out finishing remote event sink streams during shutdown")
                }
            }
        }

        server_result?;

        Ok(())
    }

    /// Run a request that does bidirectional streaming.
    ///
    /// This mostly just ensures that a client context has been sent first, and passes a client
    /// stream to `func` that converts to the correct type (or returns an error and shuts the
    /// stream down)
    async fn run_bidirectional<Req, Res, PartialRes, F>(
        &self,
        req: Request<tonic::Streaming<StreamingRequest>>,
        opts: impl StreamingCommandOptions<StreamingRequest>,
        func: F,
    ) -> Result<Response<ResponseStream>, Status>
    where
        F: for<'a> FnOnce(
                &'a ServerCommandContext,
                PartialResultDispatcher<PartialRes>,
                &ClientContext,
                StreamingRequestHandler<Req>,
            ) -> BoxFuture<'a, buck2_error::Result<Res>>
            + Send
            + 'static,
        Req: TryFrom<StreamingRequest, Error = buck2_error::Error> + Send + Sync + 'static,
        Res: Into<command_result::Result> + Send + 'static,
        PartialRes: Into<partial_result::PartialResult> + Send + 'static,
    {
        let mut req = req.into_inner();
        let init_request = match req.message().await? {
            Some(
                m @ StreamingRequest {
                    request: Some(buck2_cli_proto::streaming_request::Request::Context(_)),
                },
            ) => Ok(m),
            _ => Err(Status::failed_precondition(
                "no client context message was received",
            )),
        }?;

        let init_request = Request::new(init_request);
        let activity_sender = self.0.command_channel.clone();
        self.run_streaming(
            init_request,
            opts,
            move |ctx, partial_result_dispatcher, init_req| {
                // TODO: Use the PartialResultDispatcher instead of writing events.
                func(
                    ctx,
                    partial_result_dispatcher,
                    init_req
                        .client_context()
                        .expect("already checked for a valid context"),
                    StreamingRequestHandler::new(req, activity_sender.clone()),
                )
            },
        )
        .await
    }

    /// The body of `run_streaming`, compiled once for all commands. `#[inline(never)]` keeps
    /// it from being inlined back into each command's `run_streaming` instantiation.
    #[inline(never)]
    fn run_streaming_erased(
        &self,
        command: Box<dyn ErasedStreamingCommand>,
    ) -> BoxFuture<'_, buck2_error::Result<Response<ResponseStream>>> {
        Box::pin(async move {
            let command_start = Instant::now();

            if buck2_env!("BUCK2_TEST_FAIL_STREAMING", bool, applicability = testing).unwrap() {
                Err(buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Input,
                    "Injected client streaming error"
                ))?;
            }

            let client_ctx = command.client_context()?;
            let soft_error_context = Arc::new(SoftErrorContext::new(
                &client_ctx.buck2_hard_error,
                &client_ctx.buck2_show_soft_errors,
            )?);

            command.pre_run(self)?;

            let daemon_state = self.0.daemon_state.dupe();
            let trace_id = client_ctx.trace_id.parse()?;
            let (events, dispatch) = daemon_state.prepare_events(trace_id).await?;
            let dispatch = dispatch.with_soft_error_context(soft_error_context);
            let ActiveCommand {
                guard,
                daemon_shutdown_channel,
                state,
            } = ActiveCommand::new(&dispatch, client_ctx.sanitized_argv.clone());

            // A command that contends for the DICE graph is starting; cancel any
            // in-progress idle page-out so it yields resources back. Read-only /
            // debug commands opt out (see `triggers_idle_page_out`).
            let triggers_idle_page_out = command.triggers_idle_page_out();
            let prevents_background_cleanup = command.prevents_background_cleanup();
            if triggers_idle_page_out {
                cancel_active_page_out();
            }

            let data = daemon_state.data();
            let tenant = data.tenant_for_client_context(client_ctx).await?;
            let repo = tenant.repo.dupe();

            // The total disk space on `buck-out`, effectively fixed for the daemon's life.
            // Captured here alongside `SystemInfo` and handed to this command's
            // `PagingManager`, which pairs it with the command-end snapshot's used-disk
            // reading to gate idle page-out without a second disk stat.
            let total_disk_space_bytes = disk_space_stats(repo.paths.buck_out_path())
                .ok()
                .map(|DiskSpaceStats { total_space, .. }| total_space);

            // Fire off a system-wide event to record the memory usage of this process.
            // TODO(ezgi): add it to oneshot command too
            let system_warning_config = &repo.system_warning_config;
            dispatch.instant_event(buck2_data::SystemInfo {
                system_total_memory_bytes: Some(system_memory_stats()),
                memory_pressure_threshold_percent: system_warning_config
                    .memory_pressure_threshold_percent,
                total_disk_space_bytes,
                remaining_disk_space_threshold_gb: system_warning_config
                    .remaining_disk_space_threshold_gb,
                min_re_download_bytes_threshold: system_warning_config
                    .min_re_download_bytes_threshold,
                avg_re_download_bytes_per_sec_threshold: system_warning_config
                    .avg_re_download_bytes_per_sec_threshold,
                optin_vpn_check_targets_regex: system_warning_config
                    .optin_vpn_check_targets_regex
                    .clone(),
                enable_stable_revision_check: system_warning_config.enable_stable_revision_check,
                enable_health_check_process_isolation: system_warning_config
                    .enable_health_check_process_isolation,
                daemon_cgroup_path: {
                    #[cfg(unix)]
                    {
                        data.memory_tracker
                            .as_ref()
                            .map(|mt| mt.cgroup_tree.daemon().path().to_string())
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                },
                allprocs_cgroup_path: {
                    #[cfg(unix)]
                    {
                        data.memory_tracker
                            .as_ref()
                            .map(|mt| mt.cgroup_tree.allprocs().path().to_string())
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                },
                daemon_cgroup_slice_path: {
                    #[cfg(unix)]
                    {
                        std::fs::read_to_string("/proc/self/cgroup")
                            .ok()
                            .and_then(|s| {
                                s.lines()
                                    .find_map(|l| l.strip_prefix("0::"))
                                    .map(|p| format!("/sys/fs/cgroup{}", p))
                            })
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                },
                num_cores: Some(num_cores() as u64),
                container_memory_limit_bytes: container_memory_limit_bytes(),
            });

            // Fire off a snapshot before we start doing anything else. We use the metrics emitted here
            // as a baseline.
            let snapshot_collector =
                SnapshotCollector::new(data.dupe(), tenant.dupe(), self.0.rt.clone());
            dispatch.instant_event(Box::new(snapshot_collector.create_snapshot().await));
            let cert_state = self.0.cert_state.dupe();

            let repo_root = repo.paths.project_root().root().to_buf();
            // Spawn an async task to collect expensive info
            // We start collecting immediately, and emit the event as soon as it is ready
            let version_control_revision_collector =
                version_control_revision::spawn_version_control_collector(
                    dispatch.dupe(),
                    repo_root,
                );

            #[cfg(unix)]
            let memory_reporter = daemon_state.data.memory_tracker.as_ref().map(|t| {
                buck2_resource_control::memory_tracker::spawn_memory_reporter(
                    dispatch.dupe(),
                    t.dupe(),
                )
            });

            let cancellation = tenant.cancellation.clone();
            let resp = streaming(
                events,
                state,
                dispatch.dupe(),
                daemon_shutdown_channel,
                &cancellation,
                move |cancellations| {
                    async move {
                        let _background_cleanup_guard = if prevents_background_cleanup {
                            Some(repo.materializer.prevent_background_cleanup().await)
                        } else {
                            None
                        };
                        let result: buck2_error::Result<command_result::Result> = try {
                            let base_context = daemon_state
                                .prepare_command(tenant, dispatch.dupe(), guard)
                                .await?;

                            let client_ctx = command.client_context()?;

                            let instrumentation = command.starlark_profiler_configuration(
                                base_context.repo().paths.project_root(),
                            )?;
                            let profiling_manager = StarlarkProfilingManager::new(
                                client_ctx.profile_pattern_opts.as_ref(),
                                instrumentation,
                                &base_context.events,
                            )?;

                            let context = ServerCommandContext::new(
                                base_context,
                                client_ctx,
                                profiling_manager,
                                command.build_options(),
                                cert_state.dupe(),
                                snapshot_collector,
                                cancellations,
                                command_start,
                                total_disk_space_bytes,
                            )?;

                            let res = command.run(&context, dispatch.dupe()).await;

                            // Build failures are reported inside a successful response, so the
                            // command's own result does not say whether a deferred read failed.
                            if let Some(payload) = deferred_read_failures(
                                context.base_context.tenant.dice_manager.unsafe_dice(),
                            ) {
                                dispatch.instant_event(buck2_data::StructuredError {
                                    payload,
                                    quiet: false,
                                    task: Some(false),
                                    soft_error_category: Some(buck2_data::SoftError {
                                        category: "paging_deferred_read_failed".to_owned(),
                                        is_quiet: false,
                                        is_deprecation: false,
                                    }),
                                    daemon_in_memory_state_is_corrupted: true,
                                    ..Default::default()
                                });
                            }
                            if let Some(payload) =
                                heap_rekeys(context.base_context.tenant.dice_manager.unsafe_dice())
                            {
                                dispatch.instant_event(buck2_data::StructuredError {
                                    payload,
                                    quiet: false,
                                    task: Some(false),
                                    soft_error_category: Some(buck2_data::SoftError {
                                        category: "paging_heap_indexed_under_two_keys".to_owned(),
                                        is_quiet: false,
                                        is_deprecation: false,
                                    }),
                                    ..Default::default()
                                });
                            }

                            // Finalize the command, emitting its paging telemetry and (if
                            // eligible) scheduling an idle page-out.
                            context.finalize(triggers_idle_page_out).await?;
                            res?
                        };

                        // Do not kill the process prematurely.
                        drop(version_control_revision_collector);
                        #[cfg(unix)]
                        drop(memory_reporter);
                        match result {
                            Ok(_) => dispatch.command_result(result_to_command_result(result)),
                            Err(e) => {
                                let report = check_cert_state_for(cert_state, [&e]).await;
                                if report.cert_error().is_some() {
                                    dispatch.instant_event(buck2_data::TagEvent {
                                        tags: vec![INVALID_CERTS_TAG.to_owned()],
                                    });
                                }
                                let e = match report {
                                    CertStateReport::Valid => e,
                                    CertStateReport::Explains(err) => err.context(format!("{e:?}")),
                                    CertStateReport::Hint(err) => {
                                        dispatch.console_warning(err.to_string());
                                        e
                                    }
                                };
                                dispatch.command_result(error_to_command_result(e))
                            }
                        }
                    }
                    .boxed()
                },
                &self.0.rt,
            );
            Ok(resp)
        })
    }

    /// Runs a single command (given by the func F). Prior to running the command, calls the
    /// `opts`'s `pre_run` hook.  then bootstraps an event source and command context so that the
    /// invoked function has the ability to stream events to the caller.
    async fn run_streaming<Req, Res, PartialRes, F>(
        &self,
        req: Request<Req>,
        opts: impl StreamingCommandOptions<Req>,
        func: F,
    ) -> Result<Response<ResponseStream>, Status>
    where
        F: for<'a> FnOnce(
                &'a ServerCommandContext,
                PartialResultDispatcher<PartialRes>,
                Req,
            ) -> BoxFuture<'a, buck2_error::Result<Res>>
            + Send
            + 'static,
        Req: HasClientContext + HasBuildOptions + Send + Sync + 'static,
        Res: Into<command_result::Result> + Send + 'static,
        PartialRes: Into<partial_result::PartialResult> + Send + 'static,
    {
        self.note_command_activity(opts.daemon_activity());

        let command = Box::new(StreamingCommand {
            req: req.into_inner(),
            opts,
            func,
            _types: PhantomData,
        });

        match self.run_streaming_erased(command).await {
            Ok(resp) => Ok(resp),
            Err(e) => {
                // There is no event stream to send a hint on yet, so a user error gets the cert
                // error as context, which does not change its classification.
                let e = match check_cert_state_for(self.0.cert_state.dupe(), [&e]).await {
                    CertStateReport::Valid => e,
                    CertStateReport::Explains(err) => err.context(format!("{e:?}")),
                    CertStateReport::Hint(err) => e.context(err.to_string()),
                };
                Ok(error_to_response_stream(e))
            }
        }
    }

    async fn oneshot<
        Req,
        Res: Into<command_result::Result>,
        Fut: Future<Output = buck2_error::Result<Res>> + Send,
        F: FnOnce(Req) -> Fut,
    >(
        &self,
        req: Request<Req>,
        opts: impl OneshotCommandOptions,
        func: F,
    ) -> Result<Response<CommandResult>, Status> {
        opts.pre_run(self)?;

        let req = req.into_inner();
        let result = func(req).await;
        Ok(Response::new(result_to_command_result(result)))
    }

    /// Checks if the server is accepting requests.
    #[allow(clippy::result_large_err)]
    fn check_if_accepting_requests(&self) -> Result<(), Status> {
        if self.0.stop_accepting_requests.load(Ordering::Relaxed) {
            Err(Status::failed_precondition(
                "Failed to run command, `buckd` is shutting down soon!",
            ))
        } else {
            Ok(())
        }
    }

    fn note_command_activity(&self, activity: DaemonActivity) {
        self.0.inactivity_config.note_activity(activity);
        let _ = self.0.command_channel.unbounded_send(());
    }
}

#[allow(clippy::result_large_err)]
/// The message for the finishing command's corrupted-state report when a
/// deferred read has failed on paged-out data. A value paged back in cannot be
/// repaired once a field it kept in storage fails to read, so the daemon has to
/// be replaced, and every command that finishes before that happens says so:
/// the client only restarts a command that failed, so a report consumed by an
/// unrelated command that succeeded would be lost.
fn deferred_read_failures(dice: &Dice) -> Option<String> {
    let failures = dice
        .pagable_storage_context()?
        .get::<DeferredReadFailures>()?
        .snapshot();
    if failures.is_empty() {
        return None;
    }
    Some(format!(
        "Deferred reads of paged-out data failed: [{}]. The daemon's in-memory state cannot be repaired; restarting the daemon",
        failures.join("; ")
    ))
}

/// The report for Starlark heaps the storage indexed under a second data key
/// during this command, which should not occur and leaves page-in able to
/// deserialize a copy of a resident heap.
fn heap_rekeys(dice: &Dice) -> Option<String> {
    let rekeyed = starlark::pagable::take_heap_rekeys(dice.pagable_storage_context()?);
    (rekeyed > 0).then(|| {
        format!(
            "{rekeyed} Starlark heap(s) were indexed under a second data key, so a page-in \
             through that key deserializes a copy of a heap that is still resident"
        )
    })
}

fn convert_positive_duration(proto_duration: &prost_types::Duration) -> Result<Duration, Status> {
    if proto_duration.seconds < 0 || proto_duration.nanos < 0 {
        return Err(Status::new(
            Code::Unknown,
            format!("received invalid timeout: `{proto_duration:?}`"),
        ));
    }
    Ok(Duration::from_secs(proto_duration.seconds as u64)
        + Duration::from_nanos(proto_duration.nanos as u64))
}

/// Finishes the Build Event Service streams before a kill starts the shutdown, while the
/// daemon is still whole. A kill during a build ends in a forced shutdown that exits the
/// process without the graceful path's cleanup, and a stream left open shows as a build
/// running forever. Bounded, so a dead server cannot hold the kill hostage: by the drain
/// timeout when the client asked for one, which it waits for on top of its usual three
/// seconds, and otherwise by five seconds, of which the client waits three.
async fn finish_bes_for_kill(
    sink: Option<&dyn EventSinkWithStats>,
    bes_drain_timeout: Option<Duration>,
) -> Option<BesDrainResult> {
    match (bes_drain_timeout, sink) {
        (Some(drain_timeout), sink) => {
            Some(drain_bes(sink, DrainScope::Shutdown, drain_timeout).await)
        }
        (None, Some(sink)) => {
            match timeout(Duration::from_secs(5), sink.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!("remote event sink streams not finished cleanly: {:#}", e)
                }
                Err(_) => {
                    tracing::warn!("timed out finishing remote event sink streams before shutdown")
                }
            }
            None
        }
        (None, None) => None,
    }
}

async fn drain_bes(
    sink: Option<&dyn EventSinkWithStats>,
    scope: DrainScope,
    drain_timeout: Duration,
) -> BesDrainResult {
    let Some(sink) = sink else {
        return BesDrainResult::default();
    };
    match sink.drain(scope, drain_timeout).await {
        Ok(Some(report)) => bes_drain_result(report),
        Ok(None) => BesDrainResult::default(),
        Err(e) => {
            tracing::warn!("BES drain did not run: {:#}", e);
            BesDrainResult {
                sink_enabled: true,
                streams: Vec::new(),
                error: format!("{e:#}"),
            }
        }
    }
}

fn bes_drain_result(report: DrainReport) -> BesDrainResult {
    let streams = report
        .streams
        .into_iter()
        .map(|stream| {
            let (outcome, unacked_events, error) = match stream.outcome {
                StreamDrainOutcome::Acked => (bes_stream_drain::Outcome::Acked, 0, String::new()),
                StreamDrainOutcome::Pending { events } => {
                    (bes_stream_drain::Outcome::Pending, events, String::new())
                }
                StreamDrainOutcome::Failed { events, error } => {
                    (bes_stream_drain::Outcome::Failed, events, error)
                }
            };
            BesStreamDrain {
                invocation_id: stream.invocation_id,
                outcome: outcome as i32,
                unacked_events,
                error,
            }
        })
        .collect();
    BesDrainResult {
        sink_enabled: true,
        streams,
        error: String::new(),
    }
}

fn error_to_command_result(e: buck2_error::Error) -> CommandResult {
    CommandResult {
        result: Some(command_result::Result::Error(
            buck2_data::ErrorReport::from(&e),
        )),
    }
}

fn result_to_command_result<R: Into<command_result::Result>>(
    result: buck2_error::Result<R>,
) -> CommandResult {
    match result {
        Ok(result) => CommandResult {
            result: Some(result.into()),
        },
        Err(e) => error_to_command_result(e),
    }
}

fn error_to_command_progress(e: buck2_error::Error) -> CommandProgress {
    CommandProgress {
        progress: Some(command_progress::Progress::Result(Box::new(
            error_to_command_result(e),
        ))),
    }
}

fn error_to_response_stream(e: buck2_error::Error) -> Response<ResponseStream> {
    tonic::Response::new(Box::pin(stream::once(future::ready(Ok(
        buck2_cli_proto::MultiCommandProgress {
            messages: vec![error_to_command_progress(e)],
        },
    )))))
}

/// tonic requires the response for a streaming api to be a Sync Stream. With async/await, that requirement is really difficult
/// to meet. This simple wrapper allows us to wrap a non-Sync stream into a Sync one (the inner stream is never accessed in a
/// non-exclusive manner).
struct SyncStream<T: Stream + Send> {
    // SyncWrapper provides a Sync type that only allows (statically checked) exclusive access to
    // the underlying object, this allows using a non-Sync object where a Sync one is required
    // but is never accessed from multiple threads.
    // See https://internals.rust-lang.org/t/what-shall-sync-mean-across-an-await/12020/31
    // and https://github.com/hyperium/tonic/issues/117
    wrapped: sync_wrapper::SyncWrapper<T>,
}

impl<T: Stream + Send> Stream for SyncStream<T> {
    type Item = <T as Stream>::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // This is a safe pin projection. See https://doc.rust-lang.org/std/pin/index.html#projections-and-structural-pinning
        // Specifically see the requirements when pinning is structural for a field here: https://doc.rust-lang.org/std/pin/index.html#pinning-is-structural-for-field
        unsafe { self.map_unchecked_mut(|a| a.wrapped.get_mut()) }.poll_next(cx)
    }
}

fn pump_events(
    mut events: ChannelEventSource,
    mut state: ActiveCommandStateWriter,
    output_send: tokio::sync::mpsc::UnboundedSender<
        Result<buck2_cli_proto::CommandProgress, tonic::Status>,
    >,
) {
    // This function returns the receiving channel back to `tonic` as a streaming response.
    while let Some(next_event) = events.receive() {
        // Ignoring errors from writing to `output_send` because they occur only when
        // the receiving end of the channel is closed. This can happen, for example,
        // if Tonic drops the streaming response due the client disconnecting.
        // In these cases, ignoring the errors is intentional as no client is listening.
        match next_event {
            // The CommandResult event indicates that the spawned
            // computation won't be producing any more events.
            Event::CommandResult(result) => {
                let _ignore = output_send.send(Ok(CommandProgress {
                    progress: Some(command_progress::Progress::Result(result)),
                }));
                return;
            }
            Event::PartialResult(result) => {
                let _ignore = output_send.send(Ok(CommandProgress {
                    progress: Some(command_progress::Progress::PartialResult(Box::new(result))),
                }));
            }
            Event::Buck(buck_event) => {
                state.peek_event(&buck_event);

                let _ignore = output_send.send(Ok(CommandProgress {
                    progress: Some(command_progress::Progress::Event(buck_event.into())),
                }));
            }
        }
    }
}

/// Dispatches a request to the given function and returns a stream of responses, suitable for streaming to a client.
#[allow(clippy::mut_mut)] // select! does this internally
fn streaming<F>(
    events: ChannelEventSource,
    state: ActiveCommandStateWriter,
    dispatcher: EventDispatcher,
    daemon_shutdown_channel: oneshot::Receiver<buck2_data::DaemonShutdown>,
    cancellation_scope: &CancellationScope,
    func: F,
    rt: &Handle,
) -> Response<ResponseStream>
where
    F: for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, ()> + Send + 'static,
{
    // This function is responsible for receiving all events coming into an ChannelEventSource and reacting accordingly.
    // The function `func` is the computation that we are going to run. It communicates its success or failure using
    // control events; our first step is to spawn it.

    let trace_id = dispatcher.trace_id().dupe();
    // Created outside the task so that a task that never runs still reports.
    let report = ReportCancelledCommand {
        dispatcher: Some(dispatcher.dupe()),
        trace_id: trace_id.dupe(),
    };
    let (_spawned, cancellation) = cancellation_scope.spawn(rt, move |context| {
        async move {
            buck2_events::dispatch::with_dispatcher_async(dispatcher, func(context)).await;
            // `func` sends its own result and the event pump stops at the first one, so the guard
            // is disarmed in the same poll that `func` returns in. This relies on `func` not
            // awaiting between sending its result and returning, which holds for both callers.
            // Blocking on purpose: the test needs the task to stay in this poll.
            if let Ok(Some(ms)) = buck2_env!(
                "BUCK2_TEST_COMMAND_END_STALL_MS",
                type = u64,
                applicability = testing
            ) {
                thread::sleep(Duration::from_millis(ms));
            }
            report.disarm();
        }
        .boxed()
    });
    let cancellation = CancelCommandOnDrop(cancellation);
    let (output_send, output_recv) = tokio::sync::mpsc::unbounded_channel();

    // We run the event consumer on new non-tokio thread to avoid the consumer task from getting stuck behind
    // another tokio task in its lifo task slot. See T96012305 and https://github.com/tokio-rs/tokio/issues/4323 for more
    // information.
    let merge_task = thread_spawn("pump-events", move || {
        pump_events(events, state, output_send);
    });
    if let Err(e) = merge_task {
        return error_to_response_stream(
            buck2_error::Error::from(e).context("failed to spawn pump-events"),
        );
    };

    let events = tokio_stream::wrappers::UnboundedReceiverStream::new(output_recv);

    //
    // Note that while this is an event, we don't send it through our normal event
    // processing. The reason for that is that we dont want this event to queue behind any other
    // events in the (2) unbounded channels that form our event pipeline. So, we inject this one
    // directly where Tonic is polling for responses (which, unlike the rest of the pipeline, is
    // not unbounded, and has backpressure).

    let daemon_shutdown_stream = daemon_shutdown_channel
        .map_ok(move |shutdown| CommandProgress {
            progress: Some(command_progress::Progress::Event(Box::new(
                buck2_data::BuckEvent {
                    timestamp: Some(SystemTime::now().into()),
                    trace_id: trace_id.to_string(),
                    span_id: 0,
                    parent_id: 0,
                    data: Some(
                        buck2_data::InstantEvent {
                            data: Some(shutdown.into()),
                        }
                        .into(),
                    ),
                },
            ))),
        })
        .into_stream()
        .filter_map(|e| {
            // If the channel yields an Err, that means we didnt shut down, so for us that is
            // simply something we want to drop from the stream.
            futures::future::ready(e.ok().map(Ok))
        });

    // The stream we ultimately return is the receiving end of the channel that the above task is
    // writing to, plus the shutdown channel.
    let events = futures::stream::select(events, daemon_shutdown_stream);

    let events = MultiEventStream::new(events);

    Response::new(Box::pin(SyncStream {
        wrapped: sync_wrapper::SyncWrapper::new(DropTogether::new(events, cancellation)),
    }))
}

struct CancelCommandOnDrop(Arc<DropcancelHandle>);

impl Drop for CancelCommandOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Sends the command's result if the task is dropped before the body returns.
struct ReportCancelledCommand {
    dispatcher: Option<EventDispatcher>,
    trace_id: TraceId,
}

impl ReportCancelledCommand {
    fn disarm(mut self) {
        self.dispatcher = None;
    }
}

impl Drop for ReportCancelledCommand {
    fn drop(&mut self) {
        if let Some(dispatcher) = self.dispatcher.take() {
            dispatcher.command_result(error_to_command_result(buck2_error!(
                ErrorTag::InterruptedByDaemonShutdown,
                "Command {} was cancelled before it completed: its daemon tenant was evicted or the daemon is shutting down",
                self.trace_id
            )));
        }
    }
}

struct QueryCommandOptions {
    /// `buck2_cli_proto::ProfileMode`.
    profile_mode: Option<i32>,
}

impl OneshotCommandOptions for QueryCommandOptions {
    fn pre_run(&self, _server: &BuckdServer) -> Result<(), Status> {
        Ok(())
    }
}

impl<Req> StreamingCommandOptions<Req> for QueryCommandOptions {
    fn starlark_profiler_instrumentation_override(
        &self,
        _req: &Req,
        _project_root: &ProjectRoot,
    ) -> buck2_error::Result<StarlarkProfilerConfiguration> {
        match self.profile_mode {
            None => Ok(StarlarkProfilerConfiguration::None),
            Some(mode) => {
                let mode = buck2_cli_proto::ProfileMode::try_from(mode)
                    .internal_error("invalid profile mode enum value")?;
                Ok(StarlarkProfilerConfiguration::ProfileLoading(
                    proto_to_profile_mode(mode),
                    // We enable profiling for everything,
                    // but collect results only for subset of packages.
                    UnparsedPatternPredicate::Any,
                ))
            }
        }
    }
}

type ResponseStream =
    Pin<Box<dyn Stream<Item = Result<MultiCommandProgress, Status>> + Send + Sync>>;

#[async_trait]
impl DaemonApi for BuckdServer {
    async fn kill(&self, req: Request<KillRequest>) -> Result<Response<CommandResult>, Status> {
        struct KillRunCommandOptions;

        impl OneshotCommandOptions for KillRunCommandOptions {
            /// kill should be always available
            fn pre_run(&self, _server: &BuckdServer) -> Result<(), Status> {
                Ok(())
            }
        }

        self.oneshot(req, KillRunCommandOptions, move |req| async move {
            self.0
                .stop_accepting_requests
                .store(true, Ordering::Relaxed);

            let bes_drain_timeout = req
                .bes_drain_timeout
                .as_ref()
                .map(convert_positive_duration)
                .transpose()?
                .filter(|drain_timeout| !drain_timeout.is_zero());

            let sink = self.0.daemon_state.data().scribe_sink.dupe();
            let bes_drain = finish_bes_for_kill(sink.as_deref(), bes_drain_timeout).await;

            let timeout = req
                .timeout
                .as_ref()
                .map(convert_positive_duration)
                .transpose()?;

            let reason = buck2_data::DaemonShutdown {
                reason: req.reason,
                callers: req.callers,
            };

            self.0.daemon_shutdown.start_shutdown(reason, timeout);
            Ok(KillResponse { bes_drain })
        })
        .await
    }

    async fn ping(&self, req: Request<PingRequest>) -> Result<Response<CommandResult>, Status> {
        self.oneshot(req, DefaultCommandOptions, move |req| async move {
            if let Some(delay) = &req.delay {
                let delay = convert_positive_duration(delay)?;
                tokio::time::sleep(delay).await;
            }

            let mut payload = vec![
                0;
                req.response_payload_size
                    .try_into()
                    .buck_error_context("requested payload too large")?
            ];
            rand::rngs::SmallRng::seed_from_u64(10).fill_bytes(&mut payload);

            Ok(PingResponse { payload })
        })
        .await
    }

    async fn status(&self, req: Request<StatusRequest>) -> Result<Response<CommandResult>, Status> {
        let daemon_state = self.0.daemon_state.dupe();
        let rt = self.0.rt.clone();

        self.oneshot(req, DefaultCommandOptions, move |req| async move {
            let data = daemon_state.data();
            let metadata = data.initial_tenant_metadata();
            let snapshot = if req.snapshot
                && let Some(tenant) = data.status_tenant()
            {
                Some(
                    snapshot::SnapshotCollector::new(daemon_state.data(), tenant, rt.clone())
                        .create_snapshot()
                        .await,
                )
            } else {
                None
            };

            let extra_constraints = buck2_cli_proto::ExtraDaemonConstraints {
                trace_io_enabled: metadata.trace_io_enabled,
                materializer_state_identity: metadata
                    .materializer_state_identity
                    .as_ref()
                    .map(|i| i.to_string()),
            };

            let mut daemon_constraints = self.0.base_daemon_constraints.clone();
            daemon_constraints.extra = Some(extra_constraints);

            let valid_working_directory = daemon_state.validate_cwd().is_ok();
            let valid_buck_out_mount = daemon_state
                .validate_buck_out_mount(&metadata.paths)
                .is_ok();

            let io_provider = metadata.io_provider.clone();

            let uptime = Instant::now() - self.0.start_instant;

            let process_info = self.0.process_info.clone();

            let mut base = StatusResponse {
                process_info: Some(process_info),
                start_time: Some(self.0.start_time),
                uptime: Some(uptime.try_into()?),
                snapshot,
                daemon_constraints: Some(daemon_constraints),
                project_root: metadata.paths.project_root().to_string(),
                isolation_dir: metadata.paths.isolation().to_string(),
                forkserver_pid: match &daemon_state.data.forkserver {
                    #[cfg(unix)]
                    ForkserverAccess::Client(f) => Some(f.pid()),
                    ForkserverAccess::None => None,
                },
                supports_vpnless: Some(daemon_state.data().http_client.supports_vpnless()),
                http2: Some(daemon_state.data().http_client.http2()),
                valid_working_directory: Some(valid_working_directory),
                valid_buck_out_mount: Some(valid_buck_out_mount),
                io_provider: Some(io_provider),
                active_commands: crate::active_commands::active_commands_snapshot(),
                allprocs_cgroup_path: {
                    #[cfg(unix)]
                    {
                        daemon_state
                            .data()
                            .memory_tracker
                            .as_ref()
                            .map(|mt| mt.cgroup_tree.allprocs().path().to_string())
                    }
                    #[cfg(not(unix))]
                    {
                        None
                    }
                },
                ..Default::default()
            };

            if req.include_tokio_runtime_metrics {
                let tokio_metrics = self.0.rt.metrics();
                let metrics_response = TokioRuntimeMetrics {
                    num_workers: tokio_metrics.num_workers() as u64,
                    num_alive_tasks: tokio_metrics.num_alive_tasks() as u64,
                    global_queue_depth: tokio_metrics.global_queue_depth() as u64,
                };
                base.tokio_runtime_metrics = Some(metrics_response);
            }
            Ok(base)
        })
        .await
    }

    async fn flush_dep_files(
        &self,
        req: Request<FlushDepFilesRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        let data = self.0.daemon_state.data();
        self.oneshot(req, DefaultCommandOptions, move |req| async move {
            let tenant = data
                .tenant_for_optional_client_context(req.context.as_ref())
                .await?;
            let repo = tenant.repo.dupe();
            let FlushDepFilesRequest {
                retain_locally_produced_dep_files,
                context: _,
            } = req;
            if retain_locally_produced_dep_files {
                repo.dep_file_cache.clear_non_local();
            } else {
                // Clearing the persisted store waits for its writer thread to drain, so clear both
                // caches on the blocking pool rather than parking a runtime worker.
                let _ignored = spawn_blocking(move || {
                    repo.dep_file_cache.clear();
                    if let Some(cache) = repo.persisted_dep_file_cache.as_ref() {
                        cache.store.clear();
                    }
                })
                .await;
            }
            Ok(GenericResponse {})
        })
        .await
    }

    type HydrationStream = ResponseStream;
    async fn hydration(
        &self,
        req: Request<HydrationRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            HydrationCommandOptions,
            |context, partial_result_dispatcher, req| {
                hydration_command(context, partial_result_dispatcher, req).boxed()
            },
        )
        .await
    }

    type FileStatusStream = ResponseStream;
    async fn file_status(
        &self,
        req: Request<FileStatusRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            NonPagingCommandOptions,
            |context, partial_result_dispatcher, req| {
                file_status_command(context, partial_result_dispatcher, req).boxed()
            },
        )
        .await
    }

    type BuildStream = ResponseStream;
    async fn build(&self, req: Request<BuildRequest>) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    OTHER_SERVER_COMMANDS
                        .get()?
                        .build(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type BxlStream = ResponseStream;
    async fn bxl(&self, req: Request<BxlRequest>) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    BXL_SERVER_COMMANDS
                        .get()?
                        .bxl(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type TestStream = ResponseStream;
    async fn test(&self, req: Request<TestRequest>) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async { (TEST_COMMAND.get()?)(ctx, partial_result_dispatcher, req).await })
            },
        )
        .await
    }

    type AqueryStream = ResponseStream;
    async fn aquery(
        &self,
        req: Request<AqueryRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    QUERY_SERVER_COMMANDS
                        .get()?
                        .aquery(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type UqueryStream = ResponseStream;
    async fn uquery(
        &self,
        req: Request<UqueryRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    QUERY_SERVER_COMMANDS
                        .get()?
                        .uquery(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type CqueryStream = ResponseStream;
    async fn cquery(
        &self,
        req: Request<CqueryRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        let profile_mode = req.get_ref().profile_mode;
        self.run_streaming(
            req,
            QueryCommandOptions { profile_mode },
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    QUERY_SERVER_COMMANDS
                        .get()?
                        .cquery(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type TargetsStream = ResponseStream;
    async fn targets(
        &self,
        req: Request<TargetsRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    TARGETS_SERVER_COMMANDS
                        .get()?
                        .targets(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type CtargetsStream = ResponseStream;
    async fn ctargets(
        &self,
        req: Request<ConfiguredTargetsRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    TARGETS_SERVER_COMMANDS
                        .get()?
                        .ctargets(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type TargetsShowOutputsStream = ResponseStream;
    async fn targets_show_outputs(
        &self,
        req: Request<TargetsRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    TARGETS_SERVER_COMMANDS
                        .get()?
                        .targets_show_outputs(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type AuditStream = ResponseStream;
    async fn audit(
        &self,
        req: Request<GenericRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    AUDIT_SERVER_COMMAND
                        .get()?
                        .audit(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type StarlarkStream = ResponseStream;
    async fn starlark(
        &self,
        req: Request<GenericRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    STARLARK_SERVER_COMMAND
                        .get()?
                        .starlark(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type InstallStream = ResponseStream;
    async fn install(
        &self,
        req: Request<InstallRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    OTHER_SERVER_COMMANDS
                        .get()?
                        .install(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    async fn unstable_crash(
        &self,
        req: Request<UnstableCrashRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        self.oneshot(req, DefaultCommandOptions, move |req| async move {
            crash(req).await
        })
        .await
    }

    async fn unstable_heap_dump(
        &self,
        req: Request<UnstableHeapDumpRequest>,
    ) -> Result<Response<UnstableHeapDumpResponse>, Status> {
        self.check_if_accepting_requests()?;
        let req = req.into_inner();

        memory::write_heap_to_file(&req.destination_path)
            .map_err(|e| Status::invalid_argument(format!("failed to perform heap dump: {e}")))?;
        if let Some(test_executor_destination_path) = req.test_executor_destination_path {
            let test_executors = get_all_test_executors();
            tracing::debug!(
                "currently have {} test executor(s), dumping last one to {}",
                test_executors.len(),
                test_executor_destination_path
            );
            // TODO: Figure out a way to dump all of them and not just the last.
            if let Some(test_executor) = test_executors.last() {
                test_executor
                    .unstable_heap_dump(&test_executor_destination_path)
                    .await
                    .map_err(|e| {
                        Status::invalid_argument(format!("failed to perform heap dump: {e}"))
                    })?;
            }
        }
        Ok(Response::new(UnstableHeapDumpResponse {}))
    }

    async fn unstable_recent_alloc_dump(
        &self,
        req: Request<UnstableRecentAllocDumpRequest>,
    ) -> Result<Response<UnstableRecentAllocDumpResponse>, Status> {
        self.check_if_accepting_requests()?;
        let req = req.into_inner();

        // Producing the dump and streaming it to disk are blocking; run them off
        // the async runtime so a large dump can't stall the daemon.
        tokio::task::spawn_blocking(move || {
            memory::write_recent_alloc_to_file(&req.destination_path)
        })
        .await
        .map_err(|e| Status::internal(format!("recent alloc dump task failed: {e}")))?
        .map_err(|e| {
            // `Input`-tagged errors are unmet preconditions (jemalloc isn't the
            // allocator, or profiling / the recent-alloc ring isn't enabled);
            // anything else (mallctl or file-IO failure) is an internal error.
            let msg = format!("failed to dump recent allocations: {e:#}");
            if e.has_tag(buck2_error::ErrorTag::Input) {
                Status::failed_precondition(msg)
            } else {
                Status::internal(msg)
            }
        })?;
        Ok(Response::new(UnstableRecentAllocDumpResponse {}))
    }

    async fn unstable_flush_pgo_profile(
        &self,
        _req: Request<UnstableFlushPgoProfileRequest>,
    ) -> Result<Response<UnstableFlushPgoProfileResponse>, Status> {
        self.check_if_accepting_requests()?;
        let pgo_active = buck2_util::pgo::flush_pgo_profile();
        Ok(Response::new(UnstableFlushPgoProfileResponse {
            pgo_active,
        }))
    }

    async fn flush_bes(
        &self,
        req: Request<FlushBesRequest>,
    ) -> Result<Response<BesDrainResult>, Status> {
        self.check_if_accepting_requests()?;
        let drain_timeout = req
            .into_inner()
            .timeout
            .as_ref()
            .map(convert_positive_duration)
            .transpose()?
            .filter(|drain_timeout| !drain_timeout.is_zero())
            .ok_or_else(|| Status::invalid_argument("a BES flush needs a timeout above zero"))?;
        let sink = self.0.daemon_state.data().scribe_sink.dupe();
        Ok(Response::new(
            drain_bes(sink.as_deref(), DrainScope::FinishedCommands, drain_timeout).await,
        ))
    }

    async fn unstable_allocator_stats(
        &self,
        req: Request<UnstableAllocatorStatsRequest>,
    ) -> Result<Response<UnstableAllocatorStatsResponse>, Status> {
        self.check_if_accepting_requests()?;

        let req = req.into_inner();
        let response = if req.purge {
            memory::purge_jemalloc()
                .buck_error_context("Failed to purge jemalloc")
                .and_then(|()| memory::allocator_stats(&req.options))
        } else {
            memory::allocator_stats(&req.options)
        }
        .buck_error_context("Failed to retrieve allocator stats");

        match response {
            Ok(response) => Ok(Response::new(UnstableAllocatorStatsResponse { response })),
            Err(e) => Err(Status::invalid_argument(format!("{e:#}"))),
        }
    }

    async fn unstable_list_tenants(
        &self,
        _req: Request<UnstableListTenantsRequest>,
    ) -> Result<Response<UnstableListTenantsResponse>, Status> {
        self.check_if_accepting_requests()?;
        let tenants = self
            .0
            .daemon_state
            .data()
            .tenants()
            .into_iter()
            .map(|tenant| UnstableTenantInfo {
                project_root: tenant.key.project_root().display().to_string(),
                isolation: tenant.key.isolation().as_str().to_owned(),
                generation: tenant.generation,
                initialized: tenant.initialized,
            })
            .collect();
        Ok(Response::new(UnstableListTenantsResponse { tenants }))
    }

    async fn evict_tenant(
        &self,
        req: Request<EvictTenantRequest>,
    ) -> Result<Response<EvictTenantResponse>, Status> {
        self.check_if_accepting_requests()?;
        let context = req
            .into_inner()
            .context
            .ok_or_else(|| Status::invalid_argument("missing tenant client context"))?;
        let eviction = self
            .0
            .daemon_state
            .data()
            .evict_tenant_for_client_context(&context)
            .map_err(|e| Status::invalid_argument(format!("{e:#}")))?;
        let shutting_down = eviction
            .as_ref()
            .is_some_and(|eviction| eviction.final_tenant);
        if shutting_down {
            self.shutdown_after_final_tenant_eviction();
        }
        if let Some(eviction) = &eviction {
            eviction.completed.clone().await;
        }
        Ok(Response::new(EvictTenantResponse {
            evicted: eviction.is_some(),
            generation: eviction.as_ref().map_or(0, |eviction| eviction.generation),
            shutting_down,
        }))
    }

    async fn evict_project_tenants(
        &self,
        req: Request<EvictProjectTenantsRequest>,
    ) -> Result<Response<EvictProjectTenantsResponse>, Status> {
        self.check_if_accepting_requests()?;
        let req = req.into_inner();
        let context = req
            .context
            .ok_or_else(|| Status::invalid_argument("missing tenant client context"))?;
        let isolation = req
            .isolation
            .map(FileNameBuf::try_from)
            .transpose()
            .map_err(|e| Status::invalid_argument(format!("{e:#}")))?;
        let eviction = self
            .0
            .daemon_state
            .data()
            .evict_project_tenants_for_client_context(&context, isolation)
            .map_err(|e| Status::invalid_argument(format!("{e:#}")))?;
        if eviction.final_tenant {
            self.shutdown_after_final_tenant_eviction();
        }
        eviction.completed.await;
        Ok(Response::new(EvictProjectTenantsResponse {
            generations: eviction.generations,
            shutting_down: eviction.final_tenant,
        }))
    }

    async fn unstable_dice_dump(
        &self,
        req: Request<UnstableDiceDumpRequest>,
    ) -> Result<Response<UnstableDiceDumpResponse>, Status> {
        self.check_if_accepting_requests()?;

        let inner = req.into_inner();
        let res: buck2_error::Result<_> = try {
            let tenant = self
                .0
                .daemon_state
                .data()
                .tenant_for_optional_client_context(inner.context.as_ref())
                .await?;
            let path = inner.destination_path;
            let path = Path::new(&path);
            let format_proto =
                buck2_cli_proto::unstable_dice_dump_request::DiceDumpFormat::try_from(inner.format)
                    .buck_error_context("Invalid DICE dump format")?;

            tenant
                .spawn_dice_dump(path, format_proto)
                .await
                .with_buck_error_context(|| {
                    format!("Failed to perform dice dump to {}", path.display())
                })?;

            UnstableDiceDumpResponse {}
        };

        res.map(Response::new)
            .map_err(|e| Status::internal(format!("{e:#}")))
    }

    type AllocativeStream = ResponseStream;
    async fn allocative(
        &self,
        req: Request<AllocativeRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.check_if_accepting_requests()?;
        self.note_command_activity(DaemonActivity::Other);

        let res: buck2_error::Result<_> = try {
            let client_ctx = req.get_ref().client_context()?;
            let soft_error_context = Arc::new(SoftErrorContext::new(
                &client_ctx.buck2_hard_error,
                &client_ctx.buck2_show_soft_errors,
            )?);
            let trace_id = client_ctx
                .trace_id
                .parse()
                .map_err(buck2_error::Error::from)?;
            let (event_source, dispatcher) = self.0.daemon_state.prepare_events(trace_id).await?;
            let dispatcher = dispatcher.with_soft_error_context(soft_error_context);
            let active_command = ActiveCommand::new(&dispatcher, client_ctx.sanitized_argv.clone());
            let tenant = self
                .0
                .daemon_state
                .data()
                .tenant_for_client_context(client_ctx)
                .await?;
            (event_source, dispatcher, active_command, tenant)
        };

        let (event_source, dispatcher, active_command, tenant) = match res {
            Ok(v) => v,
            Err(e) => return Ok(error_to_response_stream(e)),
        };

        let ActiveCommand {
            guard,
            daemon_shutdown_channel,
            state,
        } = active_command;

        let this = self.0.dupe();
        let req = req.into_inner();
        let cancellation = tenant.cancellation.clone();
        Ok(streaming(
            event_source,
            state,
            dispatcher.dupe(),
            daemon_shutdown_channel,
            &cancellation,
            move |_| {
                async move {
                    let result = try {
                        spawn_allocative(
                            this,
                            tenant.repo.dupe(),
                            AbsPathBuf::try_from(req.output_path)?,
                            dispatcher.dupe(),
                        )
                        .await?;
                        AllocativeResponse {}
                    };
                    dispatcher.command_result(result_to_command_result(result));

                    drop(guard);
                }
                .boxed()
            },
            &self.0.rt,
        ))
    }

    type Profile2Stream = ResponseStream;
    async fn profile2(
        &self,
        req: Request<ProfileRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        struct ProfileCommandOptions;

        impl OneshotCommandOptions for ProfileCommandOptions {}

        impl StreamingCommandOptions<ProfileRequest> for ProfileCommandOptions {
            fn starlark_profiler_instrumentation_override(
                &self,
                req: &ProfileRequest,
                project_root: &ProjectRoot,
            ) -> buck2_error::Result<StarlarkProfilerConfiguration> {
                starlark_profiler_configuration_from_request(req, project_root)
            }
        }

        self.run_streaming(
            req,
            ProfileCommandOptions,
            |ctx, partial_result_dispatcher, req| {
                Box::pin(async {
                    match req.profile_opts.as_ref().expect("Missing profile opts") {
                        buck2_cli_proto::profile_request::ProfileOpts::TargetProfile(_) => {
                            profile_command(ctx, partial_result_dispatcher, req).await
                        }
                        buck2_cli_proto::profile_request::ProfileOpts::BxlProfile(_) => {
                            Ok(BXL_SERVER_COMMANDS
                                .get()?
                                .bxl_profile(ctx, partial_result_dispatcher, req)
                                .await?)
                        }
                    }
                })
            },
        )
        .await
    }

    type NewGenericImplStream = ResponseStream;
    async fn new_generic_impl(
        &self,
        req: Request<NewGenericRequestMessage>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            DefaultCommandOptions,
            |context, partial: PartialResultDispatcher<NoPartialResult>, req| {
                new_generic_command(context, req, partial).boxed()
            },
        )
        .await
    }

    type CleanStaleStream = ResponseStream;
    async fn clean_stale(
        &self,
        req: Request<CleanStaleRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            CleanStaleCommandOptions,
            |context, partial_result_dispatcher: PartialResultDispatcher<NoPartialResult>, req| {
                clean_stale_command(context, partial_result_dispatcher, req).boxed()
            },
        )
        .await
    }

    type LspStream = ResponseStream;
    async fn lsp(
        &self,
        req: Request<tonic::Streaming<StreamingRequest>>,
    ) -> Result<Response<Self::LspStream>, Status> {
        self.run_bidirectional(
            req,
            LspCommandOptions,
            |ctx,
             partial_result_dispatcher,
             _client_ctx,
             req: StreamingRequestHandler<LspRequest>| {
                Box::pin(async move {
                    (LSP_SERVER_COMMAND.get()?)
                        .lsp(ctx, partial_result_dispatcher, req)
                        .await
                })
            },
        )
        .await
    }

    type SubscriptionStream = ResponseStream;
    async fn subscription(
        &self,
        req: Request<tonic::Streaming<StreamingRequest>>,
    ) -> Result<Response<Self::SubscriptionStream>, Status> {
        self.run_bidirectional(
            req,
            NonPagingCommandOptions,
            |ctx,
             partial_result_dispatcher,
             _client_ctx,
             req: StreamingRequestHandler<SubscriptionRequestWrapper>| {
                run_subscription_server_command(ctx, partial_result_dispatcher, req).boxed()
            },
        )
        .await
    }

    type DapStream = ResponseStream;
    async fn dap(
        &self,
        req: Request<tonic::Streaming<StreamingRequest>>,
    ) -> Result<Response<Self::DapStream>, Status> {
        self.run_bidirectional(
            req,
            DapCommandOptions,
            |ctx,
             partial_result_dispatcher,
             _client_ctx,
             req: StreamingRequestHandler<DapRequest>| {
                run_dap_server_command(ctx, partial_result_dispatcher, req).boxed()
            },
        )
        .await
    }

    async fn set_log_filter(
        &self,
        req: Request<SetLogFilterRequest>,
    ) -> Result<Response<SetLogFilterResponse>, Status> {
        let req = req.into_inner();

        if req.daemon {
            self.0
                .log_reload_handle
                .update_log_filter(&req.log_filter)
                .buck_error_context("Error updating daemon log filter")
                .map_err(|e| Status::invalid_argument(format!("{e:#}")))?;
        }

        #[cfg(unix)]
        if req.forkserver {
            let data = self.0.daemon_state.data();
            if let ForkserverAccess::Client(forkserver) = &data.forkserver {
                forkserver
                    .set_log_filter(req.log_filter)
                    .await
                    .buck_error_context("Error forwarding daemon log filter to forkserver")
                    .map_err(|e| Status::invalid_argument(format!("{e:#}")))?;
            }
        }

        Ok(Response::new(SetLogFilterResponse {}))
    }

    type TraceIoStream = ResponseStream;
    async fn trace_io(
        &self,
        req: Request<TraceIoRequest>,
    ) -> Result<Response<ResponseStream>, Status> {
        self.run_streaming(
            req,
            NonPagingCommandOptions,
            |context, _: PartialResultDispatcher<NoPartialResult>, req| {
                trace_io_command(context, req).boxed()
            },
        )
        .await
    }
}

/// Options to configure the execution of a oneshot command (i.e. what happens in `oneshot()`).
trait OneshotCommandOptions: Send + Sync + 'static {
    #[allow(clippy::result_large_err)]
    fn pre_run(&self, server: &BuckdServer) -> Result<(), Status> {
        server.check_if_accepting_requests()
    }
}

/// Options to configure the execution of a streaming command (i.e. what happens in `run_streaming()`).
trait StreamingCommandOptions<Req>: OneshotCommandOptions {
    fn daemon_activity(&self) -> DaemonActivity {
        DaemonActivity::Other
    }

    fn starlark_profiler_instrumentation_override(
        &self,
        _req: &Req,
        _project_root: &ProjectRoot,
    ) -> buck2_error::Result<StarlarkProfilerConfiguration> {
        Ok(StarlarkProfilerConfiguration::None)
    }

    /// Whether this command takes part in the idle page-out lifecycle: it cancels
    /// an in-progress page-out when it starts, and triggers a new one when it
    /// finishes. True for commands that contend for the DICE graph; false for
    /// read-only / non-graph commands (see `NonPagingCommandOptions`,
    /// `HydrationCommandOptions`), which neither cancel nor trigger.
    fn triggers_idle_page_out(&self) -> bool {
        true
    }

    /// Whether background materializer cleanup must stop while this command runs,
    /// so that it cannot delete materialized outputs from under it.
    fn prevents_background_cleanup(&self) -> bool {
        true
    }
}

/// A streaming command with its request, result and partial result types erased.
///
/// `run_streaming` is instantiated once per command, so anything it does directly is compiled
/// once per command. Everything that does not need the concrete types lives behind this trait
/// in `run_streaming_erased`; the per-command half should stay small.
trait ErasedStreamingCommand: Send + 'static {
    fn client_context(&self) -> buck2_error::Result<&ClientContext>;

    fn build_options(&self) -> Option<&CommonBuildOptions>;

    #[allow(clippy::result_large_err)]
    fn pre_run(&self, server: &BuckdServer) -> Result<(), Status>;

    fn triggers_idle_page_out(&self) -> bool;

    fn prevents_background_cleanup(&self) -> bool;

    fn starlark_profiler_configuration(
        &self,
        project_root: &ProjectRoot,
    ) -> buck2_error::Result<StarlarkProfilerConfiguration>;

    /// Runs the command. The partial result dispatcher is created here because its type is one
    /// of the erased ones.
    fn run<'b>(
        self: Box<Self>,
        context: &'b ServerCommandContext<'_>,
        dispatch: EventDispatcher,
    ) -> BoxFuture<'b, buck2_error::Result<command_result::Result>>;
}

struct StreamingCommand<Req, Res, PartialRes, Opts, F> {
    req: Req,
    opts: Opts,
    func: F,
    _types: PhantomData<fn() -> (Res, PartialRes)>,
}

impl<Req, Res, PartialRes, Opts, F> ErasedStreamingCommand
    for StreamingCommand<Req, Res, PartialRes, Opts, F>
where
    F: for<'a> FnOnce(
            &'a ServerCommandContext,
            PartialResultDispatcher<PartialRes>,
            Req,
        ) -> BoxFuture<'a, buck2_error::Result<Res>>
        + Send
        + 'static,
    Req: HasClientContext + HasBuildOptions + Send + Sync + 'static,
    Res: Into<command_result::Result> + Send + 'static,
    PartialRes: Into<partial_result::PartialResult> + Send + 'static,
    Opts: StreamingCommandOptions<Req>,
{
    fn client_context(&self) -> buck2_error::Result<&ClientContext> {
        self.req.client_context()
    }

    fn build_options(&self) -> Option<&CommonBuildOptions> {
        self.req.build_options()
    }

    fn pre_run(&self, server: &BuckdServer) -> Result<(), Status> {
        self.opts.pre_run(server)
    }

    fn triggers_idle_page_out(&self) -> bool {
        self.opts.triggers_idle_page_out()
    }

    fn prevents_background_cleanup(&self) -> bool {
        self.opts.prevents_background_cleanup()
    }

    fn starlark_profiler_configuration(
        &self,
        project_root: &ProjectRoot,
    ) -> buck2_error::Result<StarlarkProfilerConfiguration> {
        self.opts
            .starlark_profiler_instrumentation_override(&self.req, project_root)
    }

    fn run<'b>(
        self: Box<Self>,
        context: &'b ServerCommandContext<'_>,
        dispatch: EventDispatcher,
    ) -> BoxFuture<'b, buck2_error::Result<command_result::Result>> {
        let StreamingCommand { req, func, .. } = *self;
        let partial_result_dispatcher =
            PartialResultDispatcher::new(dispatch, context.cancellation_context().into());
        func(context, partial_result_dispatcher, req)
            .map_ok(Into::into)
            .boxed()
    }
}

fn server_shutdown_signal(
    command_receiver: UnboundedReceiver<()>,
    mut shutdown_receiver: UnboundedReceiver<tokio::time::Instant>,
    inactivity_config: DaemonInactivityConfig,
    in_process: bool,
) -> (
    impl Future<Output = ()>,
    oneshot::Receiver<tokio::time::Instant>,
) {
    let (shutdown_deadline_sender, shutdown_deadline_receiver) = oneshot::channel();

    let shutdown = async move {
        let timeout = inactivity_timeout(command_receiver, inactivity_config);
        let shutdown = shutdown_receiver.next();

        futures::pin_mut!(shutdown);
        futures::pin_mut!(timeout);

        let shutdown_deadline = match futures::future::select(timeout, shutdown).await {
            futures::future::Either::Left(_) => tokio::time::Instant::now() + DEFAULT_KILL_TIMEOUT,
            futures::future::Either::Right((shutdown_deadline, _)) => shutdown_deadline
                .unwrap_or_else(|| tokio::time::Instant::now() + DEFAULT_KILL_TIMEOUT),
        };
        // The clock starts at the decision to shut down, not once the drain returns, so that the
        // deadline bounds the drain as well as the teardown that follows it.
        spawn_shutdown_watchdog(in_process, shutdown_deadline);

        let _ = shutdown_deadline_sender.send(shutdown_deadline);
    };

    (shutdown, shutdown_deadline_receiver)
}

/// Guarantee that a daemon which has announced its shutdown actually exits.
///
/// The graceful path waits for in-flight gRPC connections to drain, and a long lived streaming
/// command such as `buck2 lsp` holds one open for as long as its client lives - the inactivity
/// timer is only reset when a command *starts*, so it can fire underneath one. While the daemon
/// waits, its listener still accepts connections that are never answered, which is worse for
/// clients than being dead: a refused connection costs them milliseconds, an unanswered one costs
/// them their whole startup budget.
///
/// Runs on its own thread, so neither the drain nor a runtime that has stopped making progress
/// can starve it.
///
/// Does nothing for an in-process daemon, which has no process of its own to end: `--no-buckd`
/// runs it on a thread inside the client, and tests run it inside the test binary. Note this is
/// not the same as `--dont-daemonize`, which still gets a process to itself.
fn spawn_shutdown_watchdog(in_process: bool, deadline: tokio::time::Instant) {
    if in_process {
        return;
    }

    let deadline = deadline.into_std() + SHUTDOWN_WATCHDOG_GRACE;
    // Panicking is the point rather than a way of giving up on it: nothing else bounds the drain,
    // and the daemon's panic hook `_exit`s, so a daemon that cannot arm its watchdog dies now
    // instead of becoming one that hangs forever.
    thread_spawn("buck2-shutdown-watchdog", move || {
        thread::sleep(deadline.saturating_duration_since(std::time::Instant::now()));

        // Clients read `buckd.stderr` to explain why the daemon went away, so make sure the
        // reason is on disk before we go.
        eprintln!("Shutdown deadline exceeded, exiting");
        let _ignored = io::stderr().flush();
        buck2_util::pgo::flush_pgo_profile();

        // `_exit` rather than `exit`: the atexit chain runs alongside the threads we are
        // abandoning, so it can block on a lock one of them holds, and a watchdog that can hang
        // is no watchdog. It skips stdio flushing, hence the flushes above.
        // SAFETY: `_exit` is async-signal-safe and does not return. It ends the process without
        // touching state that this or any other thread may be using.
        unsafe { libc::_exit(1) }
    })
    .expect("Failed to spawn shutdown watchdog thread");
}

async fn inactivity_timeout(
    mut command_receiver: UnboundedReceiver<()>,
    inactivity_config: DaemonInactivityConfig,
) {
    let duration = loop {
        let duration = inactivity_config.current_timeout();
        match timeout(duration, command_receiver.next()).await {
            Ok(Some(())) => {}        // A command arrived; restart the timer.
            Ok(None) => return,       // Channel closed; exit without logging.
            Err(_) => break duration, // Timeout expired; break with the duration that fired.
        }
    };

    eprintln!(
        "inactivity timeout elapsed ({:?}), shutting down server",
        duration
    );
}

async fn certs_validation_background_job(cert_state: CertState) {
    tokio::task::spawn(async move {
        const CERTS_VALIDATION_INTERVAL: u64 = 60 * 60; // 1 hour
        loop {
            tokio::time::sleep(Duration::from_secs(CERTS_VALIDATION_INTERVAL)).await;
            let result = validate_certs().await;
            let mut valid = cert_state.state.lock().await;

            *valid = result.is_ok();
        }
    });
}

#[cfg(fbcode_build)]
pub(crate) mod eden_health {
    use std::time::Duration;

    use buck2_core::fs::project::ProjectRoot;
    use buck2_core::soft_error_or_panic;
    use buck2_eden::connection::EdenConnectionManager;
    use buck2_eden::connection::EdenDaemonIdentity;
    use buck2_eden::error::EdenDaemonRestarted;
    use buck2_eden::error::ErrorFromHangingMount;
    use buck2_eden::semaphore;
    use buck2_error::buck2_error;

    pub(crate) async fn edenfs_health_check(fb: fbinit::FacebookInit, root: ProjectRoot) {
        tokio::task::spawn(async move {
            const HEALTH_CHECK_INTERVAL: u64 = 60 * 10; // 10 minutes
            tracing::trace!(
                "spawned EdenFS health check that runs every {} seconds",
                HEALTH_CHECK_INTERVAL
            );
            // The Eden daemon identity from the previous iteration. An identity change means
            // Eden restarted underneath this daemon, invalidating cached state and file
            // handles. This catches restarts on any daemon with an Eden repo, including ones
            // whose I/O provider doesn't go through Eden's Thrift interface.
            let mut last_identity: Option<EdenDaemonIdentity> = None;
            loop {
                tokio::time::sleep(Duration::from_secs(HEALTH_CHECK_INTERVAL)).await;
                let semaphore = match semaphore::buck2_default() {
                    Ok(semaphore) => semaphore,
                    Err(e) => {
                        soft_error_or_panic!(
                            "eden_health_check_setup_failed",
                            e.context("EdenFS health check cannot be set up")
                        );
                        return;
                    }
                };
                match EdenConnectionManager::new(fb, &root, Some(semaphore)) {
                    Ok(Some(conn)) => {
                        let info = conn
                            .with_eden(|eden| {
                                tracing::trace!("running getDaemonInfo() on EdenFS service");
                                eden.getDaemonInfo()
                            })
                            .await;
                        match info {
                            Ok(info) => {
                                tracing::debug!("check determined EdenFS is not hanging");
                                let identity = EdenDaemonIdentity::from_daemon_info(&info);
                                if let Some(last) = &last_identity {
                                    if !last.is_same_daemon(&identity) {
                                        soft_error_or_panic!(
                                            "eden_restart_detected",
                                            EdenDaemonRestarted {
                                                old_pid: last.pid(),
                                                old_start_time: last.start_time(),
                                                new_pid: identity.pid(),
                                                new_start_time: identity.start_time(),
                                            }
                                            .into()
                                        );
                                    }
                                }
                                last_identity = Some(identity);
                            }
                            Err(e) if e.is_caused_by_hanging_mount() => {
                                soft_error_or_panic!(
                                    "eden_thrift_health_check_failed",
                                    buck2_error!(
                                        buck2_error::ErrorTag::Input,
                                        "check failed with: {:#}",
                                        e
                                    )
                                );
                            }
                            Err(e) => {
                                tracing::debug!("check failed with a non-hanging error: {:#}", e);
                            }
                        }
                    }
                    // Only occurs if the .eden dir doesn't exist (indicative of an unmounted mount).
                    // Ignore this case since it's not related to a hanging Eden daemon.
                    Ok(None) => {
                        tracing::debug!("failed to run check: .eden dir does not exist");
                    }
                    // Can occur for a number of reasons, including IO into the Eden mount failing.
                    // This _could_ signal a hanging mount, so we'll report a possible hang.
                    Err(e) => {
                        soft_error_or_panic!(
                            "eden_thrift_client_creation_failed",
                            buck2_error!(
                                buck2_error::ErrorTag::Input,
                                "client creation failed with: {:#}",
                                e
                            )
                        );
                    }
                }
            }
        });
    }
}

/// No-op set of command options.
struct DefaultCommandOptions;

impl OneshotCommandOptions for DefaultCommandOptions {}

impl<Req> StreamingCommandOptions<Req> for DefaultCommandOptions {}

struct CleanStaleCommandOptions;

impl OneshotCommandOptions for CleanStaleCommandOptions {}

impl<Req> StreamingCommandOptions<Req> for CleanStaleCommandOptions {
    fn daemon_activity(&self) -> DaemonActivity {
        DaemonActivity::CleanStale
    }
}

/// Options for `debug hydration`. Its subcommands manage the idle page-out
/// directly (see `hydration_command`), so starting one must not cancel it.
struct HydrationCommandOptions;

impl OneshotCommandOptions for HydrationCommandOptions {}

impl<Req> StreamingCommandOptions<Req> for HydrationCommandOptions {
    fn triggers_idle_page_out(&self) -> bool {
        false
    }
}

/// Options for commands that reach the daemon but do no DICE graph work —
/// `debug trace-io`, `debug file-status`, and `subscribe`. They don't contend
/// with a background idle page-out or need materialized outputs, so starting one
/// leaves background cleanup running.
struct NonPagingCommandOptions;

impl OneshotCommandOptions for NonPagingCommandOptions {}

impl<Req> StreamingCommandOptions<Req> for NonPagingCommandOptions {
    fn triggers_idle_page_out(&self) -> bool {
        false
    }

    fn prevents_background_cleanup(&self) -> bool {
        false
    }
}

/// Options for `lsp`. Its stream stays open for as long as the IDE client lives, so
/// holding off background cleanup would suppress scheduled clean-stale for a whole
/// IDE session rather than for the duration of a build.
struct LspCommandOptions;

impl OneshotCommandOptions for LspCommandOptions {}

impl<Req> StreamingCommandOptions<Req> for LspCommandOptions {
    fn prevents_background_cleanup(&self) -> bool {
        false
    }
}

/// Options for `dap`, long lived in the same way as `lsp` (see `LspCommandOptions`).
struct DapCommandOptions;

impl OneshotCommandOptions for DapCommandOptions {}

impl<Req> StreamingCommandOptions<Req> for DapCommandOptions {
    fn prevents_background_cleanup(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use buck2_events::BuckEvent;
    use buck2_events::EventSink;
    use futures::channel::mpsc;
    use parking_lot::Mutex;

    use super::*;
    use crate::active_commands::ActiveCommandDropGuard;

    /// These tests drive the shutdown signal to completion inside the test binary, under
    /// `tokio::time::pause`. A watchdog would sleep against the wall clock the paused deadline has
    /// no relation to, and then `_exit` the test binary out from under the other tests.
    const IN_PROCESS: bool = true;

    /// Records the `CommandResult`s sent to the event pump's channel.
    struct RecordingSink {
        send: crossbeam_channel::Sender<Event>,
        results: Arc<Mutex<Vec<CommandResult>>>,
    }

    impl EventSink for RecordingSink {
        fn send(&self, event: Event) {
            if let Event::CommandResult(result) = &event {
                self.results.lock().push((**result).clone());
            }
            let _ignored = self.send.send(event);
        }
    }

    struct StreamingCommand {
        response: Option<Response<ResponseStream>>,
        dispatcher: EventDispatcher,
        results: Arc<Mutex<Vec<CommandResult>>>,
        /// Another sender on the pump's channel, which fails once the pump has exited.
        probe: crossbeam_channel::Sender<Event>,
        _guard: ActiveCommandDropGuard,
        /// Dropping the scope would cancel the command.
        _scope: CancellationScope,
    }

    impl StreamingCommand {
        fn start(
            func: impl FnOnce(EventDispatcher) -> BoxFuture<'static, ()> + Send + 'static,
        ) -> Self {
            Self::start_in(CancellationScope::new(), func)
        }

        fn start_in(
            scope: CancellationScope,
            func: impl FnOnce(EventDispatcher) -> BoxFuture<'static, ()> + Send + 'static,
        ) -> Self {
            let (send, recv) = crossbeam_channel::unbounded();
            let results = Arc::new(Mutex::new(Vec::new()));
            let dispatcher = EventDispatcher::new(
                TraceId::new(),
                DaemonId::new(),
                RecordingSink {
                    send: send.clone(),
                    results: results.dupe(),
                },
            );
            let ActiveCommand {
                guard,
                state,
                daemon_shutdown_channel,
            } = ActiveCommand::new(&dispatcher, Vec::new());
            let body_dispatcher = dispatcher.dupe();
            let response = streaming(
                ChannelEventSource::new(recv),
                state,
                dispatcher.dupe(),
                daemon_shutdown_channel,
                &scope,
                move |_| func(body_dispatcher),
                &Handle::current(),
            );
            Self {
                response: Some(response),
                dispatcher,
                results,
                probe: send,
                _guard: guard,
                _scope: scope,
            }
        }

        async fn wait_for_pump_exit(&self) {
            let probe = || {
                Event::Buck(BuckEvent::new(
                    SystemTime::now(),
                    self.dispatcher.trace_id().dupe(),
                    None,
                    None,
                    buck2_data::SpanStartEvent {
                        data: Some(buck2_data::AnalysisStart::default().into()),
                    }
                    .into(),
                ))
            };
            tokio::time::timeout(Duration::from_secs(30), async {
                while self.probe.send(probe()).is_ok() {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the event pump should exit after the first result");
        }

        /// What tonic does when the client disconnects.
        fn drop_response(&mut self) {
            self.response = None;
        }

        fn results(&self) -> Vec<CommandResult> {
            self.results.lock().clone()
        }

        fn cancelled_message(&self) -> String {
            format!(
                "Command {} was cancelled before it completed: its daemon tenant was evicted or the daemon is shutting down",
                self.dispatcher.trace_id()
            )
        }
    }

    fn error_report(result: &CommandResult) -> &buck2_data::ErrorReport {
        match &result.result {
            Some(command_result::Result::Error(report)) => report,
            other => panic!("expected an error result, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_streaming_reports_command_dropped_before_its_result() {
        let (started_send, started_recv) = oneshot::channel();
        let (_park_send, park_recv) = oneshot::channel::<()>();
        let (dropped_send, dropped_recv) = oneshot::channel::<()>();
        let mut command = StreamingCommand::start(move |dispatcher| {
            async move {
                let _dropped = dropped_send;
                started_send.send(()).unwrap();
                let _ignored = park_recv.await;
                dispatcher.command_result(error_to_command_result(buck2_error!(
                    ErrorTag::Input,
                    "body result"
                )));
            }
            .boxed()
        });
        started_recv.await.unwrap();

        command.drop_response();
        command.wait_for_pump_exit().await;
        // The body is dropped by then, so nothing can send another result.
        assert!(dropped_recv.await.is_err());

        let results = command.results();
        assert_eq!(
            1,
            results.len(),
            "only the guard should report: {results:?}"
        );
        let report = error_report(&results[0]);
        assert_eq!(
            Some(command.cancelled_message().as_str()),
            report.message.lines().next()
        );
        assert!(
            report
                .tags
                .contains(&(ErrorTag::InterruptedByDaemonShutdown as i32))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_streaming_does_not_report_completed_command_again() {
        let (dropped_send, dropped_recv) = oneshot::channel::<()>();
        let mut command = StreamingCommand::start(move |dispatcher| {
            async move {
                let _dropped = dropped_send;
                dispatcher.command_result(error_to_command_result(buck2_error!(
                    ErrorTag::Input,
                    "body result"
                )));
            }
            .boxed()
        });
        command.wait_for_pump_exit().await;
        assert!(dropped_recv.await.is_err());

        command.drop_response();
        tokio::task::yield_now().await;

        let results = command.results();
        assert_eq!(1, results.len(), "only the body should report: {results:?}");
        assert_eq!(
            Some("body result"),
            error_report(&results[0]).message.lines().next()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_streaming_does_not_report_command_cancelled_after_its_result() {
        let (release_send, release_recv) = std::sync::mpsc::channel::<()>();
        let (dropped_send, dropped_recv) = oneshot::channel::<()>();
        let mut command = StreamingCommand::start(move |dispatcher| {
            async move {
                let _dropped = dropped_send;
                dispatcher.command_result(error_to_command_result(buck2_error!(
                    ErrorTag::Input,
                    "body result"
                )));
                // Blocks without yielding, so the cancel below arrives during the task's final
                // poll, after the body has sent its result.
                release_recv.recv().unwrap();
            }
            .boxed()
        });
        command.wait_for_pump_exit().await;

        command.drop_response();
        release_send.send(()).unwrap();
        assert!(dropped_recv.await.is_err());
        tokio::time::sleep(Duration::from_millis(500)).await;

        let results = command.results();
        assert_eq!(1, results.len(), "only the body should report: {results:?}");
        assert_eq!(
            Some("body result"),
            error_report(&results[0]).message.lines().next()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_streaming_reports_command_admitted_after_scope_closed() {
        let scope = CancellationScope::new();
        scope.cancel();
        let command = StreamingCommand::start_in(scope, |_| {
            async { unreachable!("a closed scope should not run the command") }.boxed()
        });
        command.wait_for_pump_exit().await;

        let results = command.results();
        assert_eq!(
            1,
            results.len(),
            "only the guard should report: {results:?}"
        );
        assert_eq!(
            Some(command.cancelled_message().as_str()),
            error_report(&results[0]).message.lines().next()
        );
    }

    #[tokio::test]
    async fn test_server_shutdown_custom_idle_timeout() {
        tokio::time::pause();

        let (_cmd_tx, cmd_rx) = mpsc::unbounded::<()>();
        let (_shutdown_tx, shutdown_rx) = mpsc::unbounded::<tokio::time::Instant>();
        let inactivity_config = DaemonInactivityConfig::try_new(Some(2), false)
            .expect("test inactivity config should be valid");
        let (shutdown_future, shutdown_deadline) =
            server_shutdown_signal(cmd_rx, shutdown_rx, inactivity_config, IN_PROCESS);
        futures::pin_mut!(shutdown_future);

        let result = tokio::time::timeout(Duration::from_secs(1), &mut shutdown_future).await;
        assert!(result.is_err(), "should not shut down before timeout");

        let result = tokio::time::timeout(Duration::from_secs(2), &mut shutdown_future).await;
        assert!(result.is_ok(), "should shut down after idle timeout");
        assert_eq!(
            tokio::time::Instant::now() + DEFAULT_KILL_TIMEOUT,
            shutdown_deadline.await.unwrap()
        );
    }

    #[tokio::test]
    async fn test_server_shutdown_default_idle_timeout() {
        tokio::time::pause();

        let (_cmd_tx, cmd_rx) = mpsc::unbounded::<()>();
        let (_shutdown_tx, shutdown_rx) = mpsc::unbounded::<tokio::time::Instant>();
        let inactivity_config = DaemonInactivityConfig::try_new(None, false)
            .expect("test inactivity config should be valid");
        let (shutdown_future, _) =
            server_shutdown_signal(cmd_rx, shutdown_rx, inactivity_config, IN_PROCESS);
        futures::pin_mut!(shutdown_future);

        let result = tokio::time::timeout(Duration::from_secs(3600), &mut shutdown_future).await;
        assert!(result.is_err(), "should not shut down before 4-day default");
    }

    #[tokio::test]
    async fn test_server_shutdown_command_resets_idle_timer() {
        tokio::time::pause();

        let (cmd_tx, cmd_rx) = mpsc::unbounded::<()>();
        let (_shutdown_tx, shutdown_rx) = mpsc::unbounded::<tokio::time::Instant>();
        let inactivity_config = DaemonInactivityConfig::try_new(Some(3), false)
            .expect("test inactivity config should be valid");
        let (shutdown_future, _) =
            server_shutdown_signal(cmd_rx, shutdown_rx, inactivity_config, IN_PROCESS);
        futures::pin_mut!(shutdown_future);

        tokio::time::advance(Duration::from_secs(2)).await;
        cmd_tx.unbounded_send(()).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), &mut shutdown_future).await;
        assert!(result.is_err(), "timer should have been reset by command");

        let result = tokio::time::timeout(Duration::from_secs(2), &mut shutdown_future).await;
        assert!(
            result.is_ok(),
            "should shut down after idle timeout post-reset"
        );
    }

    #[test]
    fn test_clean_stale_daemon_timeout_promotion_is_permanent() {
        let inactivity_config = DaemonInactivityConfig::try_new(None, true)
            .expect("test inactivity config should be valid");

        assert_eq!(
            CLEAN_STALE_INACTIVITY_TIMEOUT,
            inactivity_config.current_timeout()
        );
        inactivity_config.note_activity(DaemonActivity::CleanStale);
        assert_eq!(
            CLEAN_STALE_INACTIVITY_TIMEOUT,
            inactivity_config.current_timeout()
        );
        inactivity_config.note_activity(DaemonActivity::Other);
        assert_eq!(
            DEFAULT_INACTIVITY_TIMEOUT,
            inactivity_config.current_timeout()
        );
        inactivity_config.note_activity(DaemonActivity::CleanStale);
        assert_eq!(
            DEFAULT_INACTIVITY_TIMEOUT,
            inactivity_config.current_timeout()
        );
    }

    #[tokio::test]
    async fn test_clean_stale_daemon_promotion_resets_to_standard_timeout() {
        tokio::time::pause();

        let (cmd_tx, cmd_rx) = mpsc::unbounded::<()>();
        let (_shutdown_tx, shutdown_rx) = mpsc::unbounded::<tokio::time::Instant>();
        let inactivity_config = DaemonInactivityConfig::try_new(None, true)
            .expect("test inactivity config should be valid");
        let (shutdown_future, _) =
            server_shutdown_signal(cmd_rx, shutdown_rx, inactivity_config.clone(), IN_PROCESS);
        futures::pin_mut!(shutdown_future);

        let result = tokio::time::timeout(
            CLEAN_STALE_INACTIVITY_TIMEOUT - Duration::from_secs(1),
            &mut shutdown_future,
        )
        .await;
        assert!(result.is_err(), "should not shut down before short timeout");

        inactivity_config.note_activity(DaemonActivity::Other);
        cmd_tx.unbounded_send(()).unwrap();

        let result = tokio::time::timeout(Duration::from_secs(2), &mut shutdown_future).await;
        assert!(result.is_err(), "promotion should install standard timeout");
    }

    #[tokio::test]
    async fn test_server_shutdown_preserves_deadline() {
        tokio::time::pause();

        let (_cmd_tx, cmd_rx) = mpsc::unbounded::<()>();
        let (shutdown_tx, shutdown_rx) = mpsc::unbounded();
        let inactivity_config = DaemonInactivityConfig::try_new(None, false)
            .expect("test inactivity config should be valid");
        let (shutdown_future, shutdown_deadline) =
            server_shutdown_signal(cmd_rx, shutdown_rx, inactivity_config, IN_PROCESS);
        let expected_deadline = tokio::time::Instant::now() + Duration::from_secs(7);

        shutdown_tx.unbounded_send(expected_deadline).unwrap();
        shutdown_future.await;

        assert_eq!(expected_deadline, shutdown_deadline.await.unwrap());
    }

    /// `streaming` must dispatch exactly one `CommandResult` per command, however the command's
    /// completion interleaves with its cancellation. `pump_events` stops at the first result, and
    /// `ChannelEventSink` reports a control event that finds its receiver gone as a soft error,
    /// which is a panic under the hard errors these tests run with. These tests record every
    /// result dispatched, and a panic in a spawned task fails them.
    ///
    /// The cancellation primitive makes a second one easy to produce: a task whose body completes
    /// in the same poll in which a cancel arrives still resolves as cancelled, so "the task was
    /// cancelled" does not mean "the body did not report". A client that receives its result and
    /// closes its stream cancels the command exactly like that.
    mod one_result_per_command {
        use std::sync::mpsc;

        use buck2_events::EventSink;
        use buck2_wrapper_common::invocation_id::TraceId;
        use parking_lot::Mutex;

        use super::super::*;
        use crate::active_commands::ActiveCommandDropGuard;

        /// Records every `CommandResult` dispatched for a command, in front of the sink the
        /// command's client reads from.
        struct RecordingSink<S> {
            results: Arc<Mutex<Vec<CommandResult>>>,
            inner: S,
            /// Dropped with the last clone of the dispatcher, after which nothing can send.
            _held: oneshot::Sender<()>,
        }

        impl<S: EventSink> EventSink for RecordingSink<S> {
            fn send(&self, event: Event) {
                if let Event::CommandResult(result) = &event {
                    self.results.lock().push((**result).clone());
                }
                self.inner.send(event);
            }
        }

        /// A command's plumbing before `streaming` takes it: what `run_streaming_erased` sets up.
        struct CommandUnderTest {
            dispatcher: EventDispatcher,
            events: ChannelEventSource,
            active: ActiveCommand,
            results: Arc<Mutex<Vec<CommandResult>>>,
            released: oneshot::Receiver<()>,
        }

        /// What is left once the command is streaming: the results dispatched so far, and the
        /// means to wait until nothing can dispatch another.
        struct Dispatched {
            results: Arc<Mutex<Vec<CommandResult>>>,
            released: oneshot::Receiver<()>,
            guard: ActiveCommandDropGuard,
        }

        impl CommandUnderTest {
            fn new() -> Self {
                let (events, sink) = buck2_events::create_source_sink_pair();
                let results = Arc::new(Mutex::new(Vec::new()));
                let (held, released) = oneshot::channel();
                let dispatcher = EventDispatcher::new(
                    TraceId::new(),
                    DaemonId::new(),
                    RecordingSink {
                        results: results.dupe(),
                        inner: sink,
                        _held: held,
                    },
                );
                let active = ActiveCommand::new(&dispatcher, Vec::new());
                Self {
                    dispatcher,
                    events,
                    active,
                    results,
                    released,
                }
            }

            fn stream<F>(self, scope: &CancellationScope, func: F) -> (ResponseStream, Dispatched)
            where
                F: for<'a> FnOnce(&'a CancellationContext) -> BoxFuture<'a, ()> + Send + 'static,
            {
                let ActiveCommand {
                    guard,
                    daemon_shutdown_channel,
                    state,
                } = self.active;
                let response = streaming(
                    self.events,
                    state,
                    self.dispatcher,
                    daemon_shutdown_channel,
                    scope,
                    func,
                    &Handle::current(),
                );
                (
                    response.into_inner(),
                    Dispatched {
                        results: self.results,
                        released: self.released,
                        guard,
                    },
                )
            }
        }

        impl Dispatched {
            /// Waits until every holder of the command's dispatcher is gone: the command's task,
            /// whatever `streaming` spawned beside it, and the active command registry. Only then
            /// is the list of dispatched results final.
            async fn settled(self) -> Vec<CommandResult> {
                drop(self.guard);
                let _ = timeout(Duration::from_secs(60), self.released)
                    .await
                    .expect("the command's dispatcher should be released once its task is done");
                self.results.lock().clone()
            }
        }

        fn body_result() -> CommandResult {
            CommandResult {
                result: Some(command_result::Result::PingResponse(PingResponse::default())),
            }
        }

        fn is_error(result: &CommandResult) -> bool {
            matches!(result.result, Some(command_result::Result::Error(_)))
        }

        /// The result the client reads from the stream.
        async fn delivered(response: &mut ResponseStream) -> CommandResult {
            while let Some(batch) = response.next().await {
                for message in batch
                    .expect("the stream carries no transport errors")
                    .messages
                {
                    if let Some(command_progress::Progress::Result(result)) = message.progress {
                        return *result;
                    }
                }
            }
            panic!("the stream ended without a result");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_completed_command_reports_once() {
            let scope = CancellationScope::new();
            let command = CommandUnderTest::new();
            let dispatcher = command.dispatcher.dupe();
            let (mut response, dispatched) = command.stream(&scope, move |_| {
                async move { dispatcher.command_result(body_result()) }.boxed()
            });

            assert!(!is_error(&delivered(&mut response).await));
            let results = dispatched.settled().await;
            assert_eq!(1, results.len(), "{results:?}");
        }

        /// The client receives its result and closes its stream while the command's task is still
        /// inside the poll that sent it. Closing the stream cancels the task, which then resolves
        /// as cancelled even though its body reported; that must not be reported again. This is
        /// the crash that fast-reverted the 2026-11-02 buck2 bump (D129074969).
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_client_that_disconnects_after_its_result_does_not_get_a_second_one() {
            let scope = CancellationScope::new();
            let (release, released) = mpsc::channel::<()>();
            let command = CommandUnderTest::new();
            let dispatcher = command.dispatcher.dupe();
            let (mut response, dispatched) = command.stream(&scope, move |_| {
                async move {
                    dispatcher.command_result(body_result());
                    // Blocks inside the poll on purpose: the cancel has to land before the task
                    // resolves, as a client's disconnect does when it wins the race with the
                    // task's last few instructions.
                    released.recv().unwrap();
                }
                .boxed()
            });

            assert!(!is_error(&delivered(&mut response).await));
            drop(response);
            release.send(()).unwrap();

            let results = dispatched.settled().await;
            assert_eq!(1, results.len(), "{results:?}");
            assert!(!is_error(&results[0]), "{results:?}");
        }

        /// The cancel lands in the poll that reports, before the body has sent its result. The
        /// body still completes, so its result is the only one.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_cancel_during_the_reporting_poll_does_not_add_a_result() {
            let scope = CancellationScope::new();
            let (started, polling) = oneshot::channel::<()>();
            let (release, released) = mpsc::channel::<()>();
            let command = CommandUnderTest::new();
            let dispatcher = command.dispatcher.dupe();
            let (response, dispatched) = command.stream(&scope, move |_| {
                async move {
                    started.send(()).unwrap();
                    released.recv().unwrap();
                    dispatcher.command_result(body_result());
                }
                .boxed()
            });

            polling.await.unwrap();
            drop(response);
            release.send(()).unwrap();

            let results = dispatched.settled().await;
            assert_eq!(1, results.len(), "{results:?}");
            assert!(!is_error(&results[0]), "{results:?}");
        }

        /// The client disconnects while the body is pending: the body never reports, so the
        /// cancellation is the one result.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_client_that_disconnects_mid_command_gets_one_result() {
            let scope = CancellationScope::new();
            let command = CommandUnderTest::new();
            let (response, dispatched) =
                command.stream(&scope, |_| futures::future::pending::<()>().boxed());

            drop(response);

            let results = dispatched.settled().await;
            assert_eq!(1, results.len(), "{results:?}");
            assert!(is_error(&results[0]), "{results:?}");
        }

        /// The tenant is evicted while the body is pending and the client is still connected: the
        /// client is told once that its command stopped.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn an_evicted_command_tells_its_client_once() {
            let scope = CancellationScope::new();
            let command = CommandUnderTest::new();
            let (mut response, dispatched) =
                command.stream(&scope, |_| futures::future::pending::<()>().boxed());

            scope.cancel();
            assert!(is_error(&delivered(&mut response).await));
            scope.wait().await;

            let results = dispatched.settled().await;
            assert_eq!(1, results.len(), "{results:?}");
        }

        /// The tenant was evicted before the command was spawned: the body never runs, and the
        /// client is told once.
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn a_command_admitted_after_eviction_tells_its_client_once() {
            let scope = CancellationScope::new();
            scope.cancel();
            let ran = Arc::new(AtomicBool::new(false));
            let command = CommandUnderTest::new();
            let dispatcher = command.dispatcher.dupe();
            let (mut response, dispatched) = command.stream(&scope, {
                let ran = ran.dupe();
                move |_| {
                    async move {
                        ran.store(true, Ordering::SeqCst);
                        dispatcher.command_result(body_result());
                    }
                    .boxed()
                }
            });

            assert!(is_error(&delivered(&mut response).await));

            let results = dispatched.settled().await;
            assert_eq!(1, results.len(), "{results:?}");
            assert!(!ran.load(Ordering::SeqCst));
        }
    }
    #[derive(Default)]
    struct DrainRecordingSink {
        calls: std::sync::Mutex<Vec<String>>,
        drain_takes: Duration,
    }

    #[async_trait]
    impl EventSinkWithStats for DrainRecordingSink {
        fn to_event_sync(self: Arc<Self>) -> Arc<dyn buck2_events::EventSink> {
            unreachable!("a kill sends no events")
        }

        fn stats(&self) -> buck2_events::EventSinkStats {
            unreachable!("a kill reads no stats")
        }

        async fn shutdown(&self) -> buck2_error::Result<()> {
            self.calls.lock().unwrap().push("shutdown".to_owned());
            Ok(())
        }

        async fn drain(
            &self,
            scope: DrainScope,
            timeout: Duration,
        ) -> buck2_error::Result<Option<DrainReport>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("drain {scope:?} {timeout:?}"));
            tokio::time::sleep(self.drain_takes).await;
            Ok(Some(DrainReport {
                streams: vec![buck2_events::StreamDrain {
                    invocation_id: "slow".to_owned(),
                    outcome: StreamDrainOutcome::Pending { events: 3 },
                }],
            }))
        }
    }

    #[tokio::test]
    async fn a_kill_without_a_drain_timeout_finishes_the_streams_as_before() {
        let sink = DrainRecordingSink::default();
        assert_eq!(finish_bes_for_kill(Some(&sink), None).await, None);
        assert_eq!(*sink.calls.lock().unwrap(), vec!["shutdown".to_owned()]);
    }

    #[tokio::test]
    async fn a_kill_with_a_drain_timeout_answers_only_once_the_drain_ends() {
        tokio::time::pause();
        let sink = DrainRecordingSink {
            drain_takes: Duration::from_secs(7),
            ..DrainRecordingSink::default()
        };
        let started = tokio::time::Instant::now();
        let result = finish_bes_for_kill(Some(&sink), Some(Duration::from_secs(10)))
            .await
            .expect("a kill that asked for a drain gets its result");
        // Past the five seconds a kill without a drain gives the streams.
        assert!(started.elapsed() >= Duration::from_secs(7));
        assert_eq!(
            *sink.calls.lock().unwrap(),
            vec!["drain Shutdown 10s".to_owned()]
        );
        assert_eq!(
            result,
            BesDrainResult {
                sink_enabled: true,
                streams: vec![BesStreamDrain {
                    invocation_id: "slow".to_owned(),
                    outcome: bes_stream_drain::Outcome::Pending as i32,
                    unacked_events: 3,
                    error: String::new(),
                }],
                error: String::new(),
            }
        );
    }

    #[tokio::test]
    async fn a_kill_with_a_drain_timeout_and_no_sink_says_there_is_none() {
        let result = finish_bes_for_kill(None, Some(Duration::from_secs(10))).await;
        assert_eq!(result, Some(BesDrainResult::default()));
    }
}
