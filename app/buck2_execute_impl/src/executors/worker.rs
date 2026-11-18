/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use buck2_common::client_utils::get_channel_uds;
use buck2_common::client_utils::retrying;
use buck2_common::liveliness_observer::LivelinessGuard;
use buck2_common::liveliness_observer::LivelinessObserver;
use buck2_core::soft_error_or_panic;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_events::dispatch::EventDispatcher;
use buck2_execute::execute::kind::CommandExecutionKind;
use buck2_execute::execute::manager::CommandExecutionManagerExt;
use buck2_execute::execute::manager::CommandExecutionManagerWithClaim;
use buck2_execute::execute::output::CommandStdStreams;
use buck2_execute::execute::request::CommandExecutionRequest;
use buck2_execute::execute::request::WorkerId;
use buck2_execute::execute::request::WorkerSpec;
use buck2_execute::execute::result::CommandExecutionMetadata;
use buck2_execute::execute::result::CommandExecutionResult;
use buck2_execute_local::CommandResult;
use buck2_execute_local::GatherOutputStatus;
use buck2_execute_local::StdRedirectPaths;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_hash::BuckDashMap;
use buck2_hash::BuckMutInsertionMap;
use buck2_hash::BuckMutMap;
use buck2_util::time_span::TimeSpan;
use buck2_worker_proto::ExecuteCommand;
use buck2_worker_proto::ExecuteCommandStream;
use buck2_worker_proto::ExecuteResponse;
use buck2_worker_proto::ExecuteResponseStream;
use buck2_worker_proto::execute_command::EnvironmentEntry;
use buck2_worker_proto::worker_client;
use buck2_worker_proto::worker_streaming_client;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use futures::future::Shared;
use host_sharing::HostSharingBroker;
use host_sharing::HostSharingStrategy;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::Status;
use tonic::transport::Channel;

use crate::executors::local::ForkserverAccess;

// Request to the worker can contain a command to run the tests, and that command
// might explicitly spell out the lists of tests to run. In case of a large target
// and/or very lengthy test names, this part of a request can be very large.
// Response of the worker can contain a stderr of the command => also can be very large.
// To avoid unnecessarily limiting the usecases, let's use a maximum allowed value
// for the request/response.
const MAX_MESSAGE_SIZE_BYTES: usize = usize::MAX;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = WorkerInit)]
pub enum WorkerInitError {
    #[error("Worker failed to spawn: {0}")]
    SpawnFailed(String),
    #[error("Worker exited before connecting")]
    EarlyExit {
        exit_code: Option<i32>,
        stdout: String,
        stderr: String,
    },
    #[error("Worker failed to connect within `{0:.2}` seconds: {1}")]
    ConnectionTimeout(f64, String),
    /// Any error not related to worker behavior
    #[error("Error initializing worker `{0}`")]
    InternalError(buck2_error::Error),
}

#[cfg_attr(windows, allow(dead_code))]
impl WorkerInitError {
    pub(crate) fn to_command_execution_result(
        &self,
        request: &CommandExecutionRequest,
        manager: CommandExecutionManagerWithClaim,
    ) -> CommandExecutionResult {
        let worker_spec = request.worker().as_ref().unwrap();
        let execution_kind = CommandExecutionKind::LocalWorkerInit {
            command: worker_spec.exe.clone(),
            env: request.env().clone(),
        };
        let manager = manager.with_execution_kind(execution_kind.clone());

        match self {
            WorkerInitError::EarlyExit {
                exit_code,
                stdout,
                stderr,
            } => {
                let std_streams = CommandStdStreams::Local {
                    stdout: stdout.to_owned().into(),
                    stderr: stderr.to_owned().into(),
                };
                // TODO(ctolliday) this should be a new failure type (worker_init_failure), not conflated with a "command failure" which
                // implies that it is the primary command and that exit code != 0
                manager.failure(
                    execution_kind,
                    BuckMutInsertionMap::default(),
                    std_streams,
                    *exit_code,
                    CommandExecutionMetadata::empty(TimeSpan::empty_now()),
                    None,
                    false,
                )
            }
            // TODO(ctolliday) as above, use a new failure type (worker_init_failure) that indicates this is a worker initialization error.
            WorkerInitError::ConnectionTimeout(..) | WorkerInitError::SpawnFailed(..) => manager
                .failure(
                    execution_kind,
                    BuckMutInsertionMap::default(),
                    CommandStdStreams::Local {
                        stdout: Default::default(),
                        stderr: format!("Error initializing worker: {self}").into_bytes(),
                    },
                    None,
                    CommandExecutionMetadata::empty(TimeSpan::empty_now()),
                    None,
                    false,
                ),
            WorkerInitError::InternalError(error) => {
                manager.error("get_worker_failed", error.clone())
            }
        }
    }
}

#[cfg(unix)]
fn spawn_via_forkserver(
    forkserver: ForkserverAccess,
    exe: OsString,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    working_directory: AbsNormPathBuf,
    liveliness_observer: impl LivelinessObserver + 'static,
    std_redirects: &StdRedirectPaths,
    socket_path: &AbsNormPathBuf,
    graceful_shutdown_timeout_s: Option<u32>,
    dispatcher: EventDispatcher,
) -> JoinHandle<buck2_error::Result<GatherOutputStatus>> {
    use std::os::unix::ffi::OsStrExt;

    use buck2_events::dispatch::with_dispatcher_async;

    use crate::executors::local::apply_local_execution_environment;

    let ForkserverAccess::Client(forkserver) = forkserver else {
        unreachable!("Worker should not be spawned without a forkserver")
    };

    let std_redirects = std_redirects.clone();

    let socket_path = socket_path.clone();
    tokio::spawn(with_dispatcher_async(dispatcher, async move {
        let mut req = buck2_forkserver_proto::CommandRequest {
            exe: exe.as_bytes().into(),
            argv: args.into_iter().map(|s| s.as_bytes().into()).collect(),
            cwd: Some(buck2_forkserver_proto::WorkingDirectory {
                path: working_directory.as_path().as_os_str().as_bytes().into(),
            }),
            env: vec![],
            timeout: None,
            enable_miniperf: false,
            std_redirects: Some(buck2_forkserver_proto::command_request::StdRedirectPaths {
                stdout: std_redirects.stdout.to_string(),
                stderr: std_redirects.stderr.to_string(),
            }),
            // On shutdown the worker gets `SIGTERM` and this long to exit, if set; then its whole
            // process group gets `SIGKILL`. With no `command_cgroup`, that is what stops the
            // processes the worker started.
            graceful_shutdown_timeout_s,
            command_cgroup: None,
            network_access: None,
            timeout_grace_period_s: None,
            kill_detached_descendants: None,
        };
        apply_local_execution_environment(&mut req, &working_directory, env, None);
        let res = forkserver
            .execute(
                req,
                async move { liveliness_observer.while_alive().await },
                futures::stream::pending(),
            )
            .await
            .map(|CommandResult { status, .. }| status);

        // Socket is created by worker so won't exist if initialization fails.
        if fs_util::try_exists(&socket_path)? {
            // TODO(ctolliday) delete directory (after logs are moved to buck-out)
            fs_util::remove_file(&socket_path).categorize_internal()?;
        }
        res
    }))
}

#[cfg(not(unix))]
fn spawn_via_forkserver(
    _forkserver: ForkserverAccess,
    _exe: OsString,
    _args: Vec<OsString>,
    _env: Vec<(OsString, OsString)>,
    _working_directory: AbsNormPathBuf,
    _liveliness_observer: impl LivelinessObserver + 'static,
    _std_redirects: &StdRedirectPaths,
    _socket_path: &AbsNormPathBuf,
    _graceful_shutdown_timeout_s: Option<u32>,
    _dispatcher: EventDispatcher,
) -> JoinHandle<buck2_error::Result<GatherOutputStatus>> {
    unreachable!("workers should not be initialized off unix")
}

async fn spawn_worker(
    worker_id: WorkerId,
    args: Vec<String>,
    env: impl IntoIterator<Item = (OsString, OsString)>,
    streaming: bool,
    root: &AbsNormPathBuf,
    forkserver: ForkserverAccess,
    dispatcher: EventDispatcher,
    graceful_shutdown_timeout_s: Option<u32>,
) -> Result<WorkerHandle, WorkerInitError> {
    // Use fixed length path at /tmp to avoid 108 character limit for unix domain sockets
    let dir_name = format!("{}-{}", dispatcher.trace_id(), worker_id);
    let worker_dir = AbsNormPathBuf::from("/tmp/buck2_worker".to_owned())
        .map_err(WorkerInitError::InternalError)?
        .join(FileName::unchecked_new(&dir_name));
    let socket_path = worker_dir.join(FileName::unchecked_new("socket"));
    if fs_util::try_exists(&worker_dir).map_err(|e| WorkerInitError::InternalError(e.into()))? {
        return Err(WorkerInitError::InternalError(buck2_error!(
            buck2_error::ErrorTag::WorkerDirectoryExists,
            "Directory for worker already exists: {:?}",
            worker_dir
        )));
    }
    // TODO(ctolliday) put these in buck-out/<iso>/workers and only use /tmp dir for sockets
    let std_redirects = StdRedirectPaths {
        stdout: worker_dir.join(FileName::unchecked_new("stdout")),
        stderr: worker_dir.join(FileName::unchecked_new("stderr")),
    };
    fs_util::create_dir_all(&worker_dir).map_err(|e| WorkerInitError::InternalError(e.into()))?;

    tracing::info!(
        "Starting worker with logs at {}:\n$ {}\n",
        worker_dir,
        args.join(" ")
    );

    let worker_env = vec![("WORKER_SOCKET", socket_path.as_os_str())]
        .into_iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let env: Vec<(OsString, OsString)> = env.into_iter().chain(worker_env).collect();

    let (liveliness_observer, liveliness_guard) = LivelinessGuard::create();
    let diagnostics = WorkerDiagnostics::new();

    let spawn_fut = spawn_via_forkserver(
        forkserver,
        OsString::from(args[0].clone()),
        args[1..].iter().map(OsString::from).collect(),
        env.clone(),
        root.clone(),
        liveliness_observer,
        &std_redirects,
        &socket_path,
        graceful_shutdown_timeout_s,
        dispatcher,
    );

    let initial_delay = Duration::from_millis(50);
    let max_delay = Duration::from_millis(500);
    // Might want to make this configurable, and/or measure impact of worker initialization on critical path
    let timeout = Duration::from_mins(1);
    let (channel, check_exit) = {
        let socket_path = &socket_path;

        let connect = retrying(initial_delay, max_delay, timeout, move || {
            // TODO(ctolliday) T153604304
            // add handshake over grpc before returning a handle, to make sure the worker is responding
            get_channel_uds(socket_path, false)
        });

        let check_exit = async move {
            spawn_fut
                .await
                .map_err(|e| WorkerInitError::InternalError(e.into()))?
        }
        .boxed();
        futures::pin_mut!(connect);

        match futures::future::select(connect, check_exit).await {
            futures::future::Either::Left((connection_result, check_exit)) => {
                match connection_result {
                    Ok(channel) => Ok((channel, check_exit)),
                    Err(e) => Err(WorkerInitError::ConnectionTimeout(
                        timeout.as_secs_f64(),
                        e.to_string(),
                    )),
                }
            }
            futures::future::Either::Right((command_result, _)) => Err(match command_result {
                Ok(GatherOutputStatus::SpawnFailed(e)) => WorkerInitError::SpawnFailed(e),
                Ok(GatherOutputStatus::Finished { exit_code, .. }) => {
                    let stdout = fs_util::read_to_string(&std_redirects.stdout)
                        .categorize_internal()
                        .map_err(|e| WorkerInitError::InternalError(e.into()))?;
                    let stderr = fs_util::read_to_string(&std_redirects.stderr)
                        .categorize_internal()
                        .map_err(|e| WorkerInitError::InternalError(e.into()))?;
                    WorkerInitError::EarlyExit {
                        exit_code: Some(exit_code),
                        stdout,
                        stderr,
                    }
                }
                Ok(GatherOutputStatus::Cancelled | GatherOutputStatus::TimedOut(_)) => {
                    WorkerInitError::InternalError(buck2_error!(
                        buck2_error::ErrorTag::WorkerCancelled,
                        "Worker cancelled by buck"
                    ))
                }
                Err(e) => WorkerInitError::InternalError(e),
            }),
        }?
    };

    let (child_exited_observer, child_exited_guard) = LivelinessGuard::create();
    tokio::spawn(async move {
        let result = check_exit.await;
        diagnostics.exit_observed(&result);
        drop(result);
        drop(child_exited_guard);
    });

    tracing::info!("Connected to socket for spawned worker: {}", socket_path);
    let client = if streaming {
        WorkerClient::stream(channel, diagnostics)
            .await
            .map_err(|e| WorkerInitError::SpawnFailed(e.to_string()))?
    } else {
        WorkerClient::single(channel)
    };

    Ok(WorkerHandle::new(
        client,
        child_exited_observer,
        std_redirects,
        liveliness_guard,
        diagnostics,
    ))
}

type WorkerFuture = Shared<BoxFuture<'static, Result<Arc<WorkerHandle>, Arc<WorkerInitError>>>>;

pub struct WorkerPool {
    workers: Arc<parking_lot::Mutex<BuckMutMap<WorkerId, WorkerFuture>>>,
    brokers: Arc<parking_lot::Mutex<BuckMutMap<WorkerId, Arc<HostSharingBroker>>>>,
    graceful_shutdown_timeout_s: Option<u32>,
}

impl WorkerPool {
    pub fn new(graceful_shutdown_timeout_s: Option<u32>) -> WorkerPool {
        tracing::info!("Creating new WorkerPool");
        WorkerPool {
            workers: Arc::new(parking_lot::Mutex::new(BuckMutMap::default())),
            brokers: Arc::new(parking_lot::Mutex::new(BuckMutMap::default())),
            graceful_shutdown_timeout_s,
        }
    }

    pub fn get_worker_broker(&self, worker_spec: &WorkerSpec) -> Option<Arc<HostSharingBroker>> {
        let mut brokers = self.brokers.lock();
        worker_spec.concurrency.map(|concurrency| {
            brokers
                .entry(worker_spec.id)
                .or_insert_with(|| {
                    Arc::new(HostSharingBroker::new(
                        HostSharingStrategy::Fifo,
                        concurrency,
                    ))
                })
                .clone()
        })
    }

    pub fn get_or_create_worker(
        &self,
        worker_spec: &WorkerSpec,
        env: impl IntoIterator<Item = (OsString, OsString)>,
        root: &AbsNormPathBuf,
        forkserver: ForkserverAccess,
        dispatcher: EventDispatcher,
    ) -> (bool, WorkerFuture) {
        let mut workers = self.workers.lock();
        if let Some(worker_fut) = workers.get(&worker_spec.id) {
            (false, worker_fut.clone())
        } else {
            let worker_id = worker_spec.id;
            let args = worker_spec.exe.to_vec();
            let streaming = worker_spec.streaming;
            let root = root.clone();
            let env: Vec<(OsString, OsString)> = env.into_iter().collect();
            let graceful_shutdown_timeout_s = self.graceful_shutdown_timeout_s;
            let fut = async move {
                match spawn_worker(
                    worker_id,
                    args,
                    env,
                    streaming,
                    &root,
                    forkserver,
                    dispatcher,
                    graceful_shutdown_timeout_s,
                )
                .await
                {
                    Ok(worker) => Ok(Arc::new(worker)),
                    Err(e) => Err(Arc::new(e)),
                }
            }
            .boxed()
            .shared();

            workers.insert(worker_id, fut.clone());
            (true, fut)
        }
    }
}

/// Process-local, generated correlation only; not a process id or a globally unique identity.
#[derive(Clone, Copy, Dupe)]
struct WorkerDiagnostics {
    ordinal: u64,
}

impl WorkerDiagnostics {
    fn new() -> Self {
        static NEXT_ORDINAL: AtomicU64 = AtomicU64::new(1);
        Self {
            ordinal: NEXT_ORDINAL.fetch_add(1, Ordering::Relaxed),
        }
    }

    fn exit_observed(self, result: &buck2_error::Result<GatherOutputStatus>) {
        let kind = match result {
            Ok(GatherOutputStatus::Finished { exit_code, .. }) => {
                tracing::info!(
                    target: "buck2_worker_diagnostic",
                    worker_ordinal = self.ordinal,
                    event = "exit-result-observed",
                    exit_kind = "finished",
                    exit_code = *exit_code,
                    "Worker diagnostic receipt"
                );
                return;
            }
            Ok(GatherOutputStatus::SpawnFailed(_)) => WorkerExitKind::SpawnFailed,
            Ok(GatherOutputStatus::Cancelled) => WorkerExitKind::Cancelled,
            Ok(GatherOutputStatus::TimedOut(_)) => WorkerExitKind::TimedOut,
            Err(_) => WorkerExitKind::ObservationError,
        };
        tracing::info!(
            target: "buck2_worker_diagnostic",
            worker_ordinal = self.ordinal,
            event = "exit-result-observed",
            exit_kind = %kind,
            "Worker diagnostic receipt"
        );
    }

    fn handle_drop_start(self) {
        tracing::info!(
            target: "buck2_worker_diagnostic",
            worker_ordinal = self.ordinal,
            event = "handle-drop-start",
            "Worker diagnostic receipt"
        );
    }
}

#[derive(derive_more::Display)]
enum WorkerExitKind {
    #[display("spawn-failed")]
    SpawnFailed,
    #[display("cancelled")]
    Cancelled,
    #[display("timed-out")]
    TimedOut,
    #[display("observation-error")]
    ObservationError,
}

type WorkerResponseWaiters = BuckDashMap<u64, tokio::sync::oneshot::Sender<ExecuteResponseStream>>;

struct WorkerResponseReader {
    diagnostics: WorkerDiagnostics,
    sequence: u64,
}

impl WorkerResponseReader {
    fn new(diagnostics: WorkerDiagnostics) -> Self {
        Self {
            diagnostics,
            sequence: 0,
        }
    }

    fn response(
        &mut self,
        response: Result<ExecuteResponseStream, Status>,
        waiters: &WorkerResponseWaiters,
    ) {
        self.sequence += 1;
        match response {
            Ok(response) => {
                tracing::info!(
                    target: "buck2_worker_diagnostic",
                    worker_ordinal = self.diagnostics.ordinal,
                    event = "response-item-observed",
                    reader_sequence = self.sequence,
                    request_id = response.id,
                    "Worker diagnostic receipt"
                );
                match waiters.remove(&response.id) {
                    Some(waiter) => {
                        // The waiter is gone when its action was cancelled.
                        let _cancelled = waiter.1.send(response);
                    }
                    None => {
                        soft_error_or_panic!(
                            "worker_response_without_waiter",
                            buck2_error::internal_error!(
                                "Missing waiter for streaming worker response {}",
                                response.id
                            )
                        );
                    }
                };
            }
            Err(e) => {
                tracing::debug!(error = e.to_string(), "Response error in worker stream");
                if tracing::enabled!(target: "buck2_worker_diagnostic", tracing::Level::INFO) {
                    // Independent sampled observations: abandoned receivers remain registered,
                    // and the map/receivers can race either count. These are not active-job counts.
                    let registered_entries_observed = waiters.len();
                    let open_receiver_entries_observed = waiters
                        .iter()
                        .filter(|entry| !entry.value().is_closed())
                        .count();
                    tracing::info!(
                        target: "buck2_worker_diagnostic",
                        worker_ordinal = self.diagnostics.ordinal,
                        event = "response-error-observed",
                        reader_sequence = self.sequence,
                        tonic_code = e.code() as i32,
                        registered_entries_observed,
                        open_receiver_entries_observed,
                        "Worker diagnostic receipt"
                    );
                }
            }
        }
    }

    fn eof(&self) {
        tracing::info!(
            target: "buck2_worker_diagnostic",
            worker_ordinal = self.diagnostics.ordinal,
            event = "response-stream-eof-observed",
            reader_sequence = self.sequence,
            "Worker diagnostic receipt"
        );
    }
}

#[derive(derive_more::Display)]
enum WorkerRequestOutcome {
    #[display("send-failed")]
    SendFailed,
    #[display("response-returned")]
    ResponseReturned,
    #[display("response-body-missing")]
    ResponseBodyMissing,
    #[display("receive-closed")]
    ReceiveClosed,
    #[display("stream-closed")]
    StreamClosed,
    #[display("future-dropped-before-return")]
    FutureDroppedBeforeReturn,
}

/// Emits an observation only. In particular, it neither owns nor removes a waiter.
struct WorkerRequestReceipt {
    diagnostics: WorkerDiagnostics,
    id: u64,
    outcome: WorkerRequestOutcome,
}

impl WorkerRequestReceipt {
    fn new(diagnostics: WorkerDiagnostics, id: u64) -> Self {
        tracing::info!(
            target: "buck2_worker_diagnostic",
            worker_ordinal = diagnostics.ordinal,
            event = "request-wait-registered",
            request_id = id,
            "Worker diagnostic receipt"
        );
        Self {
            diagnostics,
            id,
            outcome: WorkerRequestOutcome::FutureDroppedBeforeReturn,
        }
    }
}

impl Drop for WorkerRequestReceipt {
    fn drop(&mut self) {
        tracing::info!(
            target: "buck2_worker_diagnostic",
            worker_ordinal = self.diagnostics.ordinal,
            event = "request-wait-ended",
            request_id = self.id,
            outcome = %self.outcome,
            "Worker diagnostic receipt"
        );
    }
}

#[derive(Clone)]
enum WorkerClient {
    Single(worker_client::WorkerClient<Channel>),
    Stream {
        ids: Arc<AtomicU64>,
        stream: UnboundedSender<ExecuteCommandStream>,
        stream_closed_observer: Arc<dyn LivelinessObserver>,
        waiters: Arc<WorkerResponseWaiters>,
        diagnostics: WorkerDiagnostics,
    },
}

impl WorkerClient {
    fn single(channel: Channel) -> Self {
        Self::Single(
            worker_client::WorkerClient::new(channel)
                .max_encoding_message_size(MAX_MESSAGE_SIZE_BYTES)
                .max_decoding_message_size(MAX_MESSAGE_SIZE_BYTES),
        )
    }

    async fn stream(channel: Channel, diagnostics: WorkerDiagnostics) -> Result<Self, Status> {
        let mut client = worker_streaming_client::WorkerStreamingClient::new(channel)
            .max_encoding_message_size(MAX_MESSAGE_SIZE_BYTES)
            .max_decoding_message_size(MAX_MESSAGE_SIZE_BYTES);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let stream = client
            .execute_stream(tonic::Request::new(UnboundedReceiverStream::new(rx)))
            .await?;
        let waiters: Arc<WorkerResponseWaiters> = Default::default();
        let (stream_closed_observer, stream_closed_guard) = LivelinessGuard::create();
        {
            let waiters = waiters.dupe();
            tokio::spawn(async move {
                use futures::StreamExt;

                let mut stream = stream.into_inner();
                let mut reader = WorkerResponseReader::new(diagnostics);
                while let Some(response) = stream.next().await {
                    reader.response(response, &waiters);
                }
                reader.eof();
                drop(stream_closed_guard);
            });
        }
        Ok(Self::Stream {
            ids: Default::default(),
            stream: tx,
            stream_closed_observer,
            waiters,
            diagnostics,
        })
    }

    async fn execute(&mut self, request: ExecuteCommand) -> buck2_error::Result<ExecuteResponse> {
        match self {
            Self::Single(client) => Self::execute_with_retry(client, request).await,
            Self::Stream {
                ids,
                stream,
                stream_closed_observer,
                waiters,
                diagnostics,
            } => {
                let id = ids.fetch_add(1, Ordering::Acquire);
                let req = ExecuteCommandStream {
                    request: Some(request),
                    id,
                };
                let (tx, rx) = tokio::sync::oneshot::channel();
                waiters.insert(id, tx);
                let mut receipt = WorkerRequestReceipt::new(*diagnostics, id);
                if let Err(err) = stream.send(req) {
                    receipt.outcome = WorkerRequestOutcome::SendFailed;
                    return Err(err.into());
                }
                tokio::select! {
                    // A real answer beats a stream that closed right after it.
                    biased;
                    response = rx => {
                        receipt.outcome = match &response {
                            Ok(response) if response.response.is_some() => WorkerRequestOutcome::ResponseReturned,
                            Ok(_) => WorkerRequestOutcome::ResponseBodyMissing,
                            Err(_) => WorkerRequestOutcome::ReceiveClosed,
                        };
                        // `response` is a proto3 message field, so a worker can leave it out.
                        response?.response.ok_or_else(|| {
                            buck2_error::buck2_error!(
                                ErrorTag::Tier0,
                                "Worker sent a response without a body for request {}",
                                id
                            )
                        })
                    }
                    _ = stream_closed_observer.while_alive() => {
                        receipt.outcome = WorkerRequestOutcome::StreamClosed;
                        Err(buck2_error::buck2_error!(ErrorTag::Tier0, "Stream closed while waiting for response"))
                    },
                }
            }
        }
    }

    async fn execute_with_retry(
        client: &mut worker_client::WorkerClient<Channel>,
        request: ExecuteCommand,
    ) -> buck2_error::Result<ExecuteResponse> {
        use tokio_retry::strategy::ExponentialBackoff;

        let retry_delays = ExponentialBackoff::from_millis(100)
            .max_delay(Duration::from_millis(500))
            .take(5);

        let mut last_err = None;
        for delay in std::iter::once(Duration::ZERO).chain(retry_delays) {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            match client.execute(request.clone()).await {
                Ok(response) => return Ok(response.into_inner()),
                Err(status) if status.code() == tonic::Code::Unavailable => {
                    tracing::debug!("Worker connection unavailable, retrying: {:?}", status);
                    last_err = Some(status);
                }
                Err(status) => return Err(status.into()),
            }
        }

        Err(last_err
            .expect("retry loop must have run at least once")
            .into())
    }
}

pub struct WorkerHandle {
    client: WorkerClient,
    child_exited_observer: Arc<dyn LivelinessObserver>,
    std_redirects: StdRedirectPaths,
    _liveliness_guard: LivelinessGuard,
    diagnostics: WorkerDiagnostics,
}

impl WorkerHandle {
    fn new(
        client: WorkerClient,
        child_exited_observer: Arc<dyn LivelinessObserver>,
        std_redirects: StdRedirectPaths,
        liveliness_guard: LivelinessGuard,
        diagnostics: WorkerDiagnostics,
    ) -> Self {
        Self {
            client,
            child_exited_observer,
            std_redirects,
            _liveliness_guard: liveliness_guard,
            diagnostics,
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.diagnostics.handle_drop_start();
    }
}

#[cfg(unix)]
fn env_entries(env: &[(OsString, OsString)]) -> Vec<EnvironmentEntry> {
    use std::os::unix::ffi::OsStrExt;
    env.iter()
        .map(|(k, v)| EnvironmentEntry {
            key: k.as_bytes().into(),
            value: v.as_bytes().into(),
        })
        .collect()
}

#[cfg(not(unix))]
fn env_entries(_env: &[(OsString, OsString)]) -> Vec<EnvironmentEntry> {
    unreachable!("worker should not exist off unix")
}

/// How long buck2 waits past a command's timeout before enforcing it itself.
///
/// The worker receives the timeout unchanged and is expected to enforce it:
/// killing the command, collecting its stderr and answering with
/// `timed_out_after_s`. That response is the better diagnostic, so buck2's own
/// backstop deliberately comes second and only fires when the worker does not
/// answer at all. Short because it only has to cover a kill and a reply, not
/// the command itself.
const WORKER_TIMEOUT_GRACE: Duration = Duration::from_secs(5);

impl WorkerHandle {
    /// Send a command to the worker and wait for its response.
    ///
    /// Returns early, without a response, if `liveliness_observer` reports the
    /// command is no longer alive (cancellation, or `--overall-timeout`
    /// expiring), or if `timeout` passes without the worker enforcing it. The
    /// worker is not told to stop on the early-return paths and may still be
    /// running the command.
    pub async fn exec_cmd(
        &self,
        args: &[String],
        env: Vec<(OsString, OsString)>,
        timeout: Option<Duration>,
        liveliness_observer: &dyn LivelinessObserver,
    ) -> CommandResult {
        tracing::info!(
            "Sending worker command:\nExecuteCommand {{ argv: {:?}, env: {:?} }}\n",
            args,
            env,
        );
        let argv: Vec<Vec<u8>> = args.iter().map(|s| s.as_str().into()).collect();
        let env: Vec<EnvironmentEntry> = env_entries(&env);

        let request = ExecuteCommand {
            argv,
            env,
            // Whole seconds on the wire: round up rather than down to 0.
            timeout_s: timeout.map(|t| t.as_secs() + u64::from(t.subsec_nanos() > 0)),
        };

        // The worker has the same deadline and is expected to reach it first,
        // answering with the command's stderr. This only fires when it does not,
        // hence the grace: the result then carries no output, but a reported
        // timeout beats waiting forever. It reports the timeout that was
        // exceeded, not the grace-extended wait.
        let timeout_backstop = async move {
            match timeout {
                Some(timeout) => {
                    tokio::time::sleep(timeout.saturating_add(WORKER_TIMEOUT_GRACE)).await;
                    timeout
                }
                None => std::future::pending().await,
            }
        };

        let mut client = self.client.clone();
        let (status, stdout, stderr) = tokio::select! {
            // Ordered most-informative first, because the default random order
            // would sometimes discard the better outcome. A real response beats
            // everything; worker death names the worker's stdout/stderr logs, so
            // it says *why* the command ended; this command blowing its own
            // budget is more specific than the whole run going away.
            biased;

            response = client.execute(request) => {
                match response {
                    Ok(exec_response) => {
                        tracing::info!("Worker response:\n{:?}\n", exec_response);
                        if let Some(timeout) = exec_response.timed_out_after_s {
                            (
                                GatherOutputStatus::TimedOut(Duration::from_secs(timeout)),
                                vec![],
                                exec_response.stderr.into(),
                            )
                        } else {
                            (
                                GatherOutputStatus::Finished {
                                    exit_code: exec_response.exit_code,
                                    execution_stats: None,
                                },
                                vec![],
                                exec_response.stderr.into(),
                            )
                        }
                    }
                    Err(err) => {
                        (
                            GatherOutputStatus::SpawnFailed(format!(
                                "Error sending ExecuteCommand to worker: {:?}, see worker logs:\n{}\n{}",
                                err, self.std_redirects.stdout, self.std_redirects.stderr,
                            )),
                            vec![],
                            vec![],
                        )
                    }
                }
            }
            _ = self.child_exited_observer.while_alive() => {
                (
                    GatherOutputStatus::SpawnFailed(format!(
                        "Worker exited while running command, see worker logs:\n{}\n{}",
                        self.std_redirects.stdout, self.std_redirects.stderr,
                    )),
                    vec![],
                    vec![],
                )
            }
            timeout = timeout_backstop => {
                (GatherOutputStatus::TimedOut(timeout), vec![], vec![])
            }
            _ = liveliness_observer.while_alive() => {
                (GatherOutputStatus::Cancelled, vec![], vec![])
            }
        };

        CommandResult {
            status,
            stdout,
            stderr,
            cgroup_result: None,
            orphan_processes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::Ordering;

    use buck2_worker_proto::ExecuteCommand;
    use buck2_worker_proto::ExecuteEvent;
    use buck2_worker_proto::ExecuteResponse;
    use buck2_worker_proto::worker_client;
    use buck2_worker_proto::worker_server::Worker;
    use buck2_worker_proto::worker_server::WorkerServer;
    use tonic::Request;
    use tonic::Response;
    use tonic::Status;
    use tonic::transport::Channel;
    use tonic::transport::Server;

    use super::WorkerClient;

    pub(super) struct MockWorker {
        pub(super) attempts: Arc<AtomicU32>,
        pub(super) fail_until: u32,
        pub(super) fail_code: tonic::Code,
    }

    #[tonic::async_trait]
    impl Worker for MockWorker {
        async fn execute(
            &self,
            _req: Request<ExecuteCommand>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            let n = self.attempts.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_until {
                Err(Status::new(self.fail_code, "test error"))
            } else {
                Ok(Response::new(ExecuteResponse {
                    exit_code: 0,
                    stderr: String::new(),
                    timed_out_after_s: None,
                }))
            }
        }

        async fn exec(
            &self,
            _req: Request<tonic::Streaming<ExecuteEvent>>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            unimplemented!()
        }
    }

    /// Serve `worker` on a loopback port and return a client connected to it.
    pub(super) async fn start_server<W: Worker + 'static>(
        worker: W,
    ) -> (
        worker_client::WorkerClient<Channel>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            Server::builder()
                .add_service(WorkerServer::new(worker))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        let client = worker_client::WorkerClient::connect(format!("http://{}", addr))
            .await
            .unwrap();
        (client, handle)
    }

    fn empty_request() -> ExecuteCommand {
        ExecuteCommand {
            argv: vec![],
            env: vec![],
            timeout_s: None,
        }
    }

    #[tokio::test]
    async fn test_retry_succeeds_immediately() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (mut client, _server) = start_server(MockWorker {
            attempts: attempts.clone(),
            fail_until: 0,
            fail_code: tonic::Code::Unavailable,
        })
        .await;
        let result = WorkerClient::execute_with_retry(&mut client, empty_request()).await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_recovers_after_transient_unavailable() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (mut client, _server) = start_server(MockWorker {
            attempts: attempts.clone(),
            fail_until: 1,
            fail_code: tonic::Code::Unavailable,
        })
        .await;
        let result = WorkerClient::execute_with_retry(&mut client, empty_request()).await;
        assert!(result.is_ok());
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_retry_does_not_retry_non_unavailable_errors() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (mut client, _server) = start_server(MockWorker {
            attempts: attempts.clone(),
            fail_until: 100,
            fail_code: tonic::Code::Internal,
        })
        .await;
        let result = WorkerClient::execute_with_retry(&mut client, empty_request()).await;
        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "non-Unavailable errors must not be retried"
        );
    }

    #[tokio::test]
    async fn test_retry_gives_up_after_max_retries() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (mut client, _server) = start_server(MockWorker {
            attempts: attempts.clone(),
            fail_until: 100,
            fail_code: tonic::Code::Unavailable,
        })
        .await;
        let result = WorkerClient::execute_with_retry(&mut client, empty_request()).await;
        assert!(result.is_err());
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            6,
            "initial attempt + 5 retries"
        );
    }
}

/// Tests that drive a [`WorkerHandle`].
///
/// Unix-only, because workers are: `spawn_via_forkserver` and `env_entries` are
/// both `unreachable!()` off unix, and `AbsNormPathBuf` rejects the `/tmp` paths
/// these use. Kept apart from `tests` so neither needs per-item `cfg` attributes.
#[cfg(all(test, unix))]
mod worker_handle_tests {
    use std::fmt::Debug;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering;
    use std::task::Context;
    use std::task::Poll;
    use std::time::Duration;

    use buck2_common::liveliness_observer::LivelinessGuard;
    use buck2_common::liveliness_observer::NoopLivelinessObserver;
    use buck2_execute_local::GatherOutputStatus;
    use buck2_execute_local::StdRedirectPaths;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_hash::BuckMutMap;
    use buck2_worker_proto::ExecuteCommand;
    use buck2_worker_proto::ExecuteCommandStream;
    use buck2_worker_proto::ExecuteEvent;
    use buck2_worker_proto::ExecuteResponse;
    use buck2_worker_proto::ExecuteResponseStream;
    use buck2_worker_proto::worker_client;
    use buck2_worker_proto::worker_server::Worker;
    use buck2_worker_proto::worker_streaming_server::WorkerStreaming;
    use buck2_worker_proto::worker_streaming_server::WorkerStreamingServer;
    use dupe::Dupe;
    use futures::Stream;
    use futures::StreamExt;
    use futures::task::noop_waker_ref;
    use tonic::Request;
    use tonic::Response;
    use tonic::Status;
    use tonic::transport::Channel;
    use tonic::transport::Server;
    use tracing::Event;
    use tracing::Metadata;
    use tracing::Subscriber;
    use tracing::field::Field;
    use tracing::field::Visit;
    use tracing::span::Attributes;
    use tracing::span::Id;
    use tracing::span::Record;

    use super::WorkerClient;
    use super::WorkerDiagnostics;
    use super::WorkerHandle;
    use super::WorkerResponseReader;
    use super::WorkerResponseWaiters;
    use super::tests::MockWorker;
    use super::tests::start_server;

    /// A worker that accepts the request and then never answers — the shape that
    /// pins a buck2 command for the whole of its Sandcastle step.
    struct HangingWorker;

    #[tonic::async_trait]
    impl Worker for HangingWorker {
        async fn execute(
            &self,
            _req: Request<ExecuteCommand>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            // `pending`, not `sleep`: must hang even under a paused clock.
            std::future::pending().await
        }

        async fn exec(
            &self,
            _req: Request<tonic::Streaming<ExecuteEvent>>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            unimplemented!()
        }
    }

    /// A `WorkerHandle` over `client`, with a live child so the
    /// `child_exited_observer` arm of `exec_cmd` cannot fire — leaving the arm
    /// under test racing only the worker's response.
    fn worker_handle(
        client: worker_client::WorkerClient<Channel>,
    ) -> (WorkerHandle, LivelinessGuard) {
        let (child_exited_observer, child_alive_guard) = LivelinessGuard::create();
        let (_, handle_guard) = LivelinessGuard::create();
        let path = |name: &str| {
            AbsNormPathBuf::from(format!("/tmp/buck2_worker_test/{name}")).expect("absolute path")
        };
        let handle = WorkerHandle::new(
            WorkerClient::Single(client),
            child_exited_observer,
            StdRedirectPaths {
                stdout: path("stdout"),
                stderr: path("stderr"),
            },
            handle_guard,
            WorkerDiagnostics::new(),
        );
        (handle, child_alive_guard)
    }

    #[tokio::test]
    async fn test_exec_cmd_returns_cancelled_when_liveliness_expires() {
        // `--overall-timeout` firing (or any cancellation) must get the command
        // back. Without the liveliness arm this call never returns and the whole
        // buck2 command hangs until its caller SIGKILLs it.
        let (client, _server) = start_server(HangingWorker).await;
        let (handle, _child_alive) = worker_handle(client);

        let (cancelled_observer, guard) = LivelinessGuard::create();
        drop(guard);

        let result = handle
            .exec_cmd(&[], vec![], None, cancelled_observer.as_ref())
            .await;

        assert!(
            matches!(result.status, GatherOutputStatus::Cancelled),
            "expected Cancelled, got {:?}",
            result.status
        );
    }

    #[tokio::test]
    async fn test_exec_cmd_returns_the_response_while_the_observer_is_alive() {
        // The counterpart to the test above: while the command is still alive the
        // liveliness arm must not fire, and a worker that answers must have its
        // answer returned. Without this, an arm that resolved eagerly — or a
        // `while_alive()` that reported dead immediately — would pass the suite.
        let attempts = Arc::new(AtomicU32::new(0));
        let (client, _server) = start_server(MockWorker {
            attempts,
            fail_until: 0,
            fail_code: tonic::Code::Unavailable,
        })
        .await;
        let (handle, _child_alive) = worker_handle(client);

        let (observer, _alive_guard) = LivelinessGuard::create();

        let result = handle.exec_cmd(&[], vec![], None, observer.as_ref()).await;

        assert!(
            matches!(
                result.status,
                GatherOutputStatus::Finished { exit_code: 0, .. }
            ),
            "expected the worker's response, got {:?}",
            result.status
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_exec_cmd_times_out_when_worker_never_responds() {
        // `timeout_s` is only advisory to the worker, so buck2 has to enforce it
        // too — otherwise a worker that never replies (e.g. one blocked acquiring
        // a device) outlives every timeout in the stack, including the tpx
        // listing timeout that should have ended the command minutes earlier.
        let (client, _server) = start_server(HangingWorker).await;
        let (handle, _child_alive) = worker_handle(client);

        let timeout = Duration::from_secs(60);
        let result = handle
            .exec_cmd(
                &[],
                vec![],
                Some(timeout),
                NoopLivelinessObserver::create().as_ref(),
            )
            .await;

        assert!(
            matches!(result.status, GatherOutputStatus::TimedOut(d) if d == timeout),
            "expected TimedOut({timeout:?}), got {:?}",
            result.status
        );
    }

    /// Records the `timeout_s` it was handed, then answers successfully.
    struct TimeoutRecordingWorker {
        seen_timeout_s: Arc<AtomicU64>,
    }

    #[tonic::async_trait]
    impl Worker for TimeoutRecordingWorker {
        async fn execute(
            &self,
            req: Request<ExecuteCommand>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            self.seen_timeout_s
                .store(req.into_inner().timeout_s.unwrap_or(0), Ordering::SeqCst);
            Ok(Response::new(ExecuteResponse {
                exit_code: 0,
                stderr: String::new(),
                timed_out_after_s: None,
            }))
        }

        async fn exec(
            &self,
            _req: Request<tonic::Streaming<ExecuteEvent>>,
        ) -> Result<Response<ExecuteResponse>, Status> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn test_exec_cmd_sends_the_worker_the_configured_timeout() {
        // The worker enforces the timeout it is given, and buck2's backstop only
        // covers the case where it does not answer at all. So the worker has to see
        // the configured value exactly -- buck2 buys itself room by waiting longer,
        // not by asking for less.
        let seen_timeout_s = Arc::new(AtomicU64::new(0));
        let (client, _server) = start_server(TimeoutRecordingWorker {
            seen_timeout_s: seen_timeout_s.clone(),
        })
        .await;
        let (handle, _child_alive) = worker_handle(client);
        let (observer, _alive_guard) = LivelinessGuard::create();

        let timeout = Duration::from_secs(60);
        handle
            .exec_cmd(&[], vec![], Some(timeout), observer.as_ref())
            .await;

        assert_eq!(
            seen_timeout_s.load(Ordering::SeqCst),
            timeout.as_secs(),
            "the worker must be given the timeout unchanged"
        );
    }

    type ResponseStream =
        Pin<Box<dyn Stream<Item = Result<ExecuteResponseStream, Status>> + Send + 'static>>;

    /// Answers every request with its id but no `response` body.
    struct NoBodyStreamingWorker;

    #[tonic::async_trait]
    impl WorkerStreaming for NoBodyStreamingWorker {
        type ExecuteStreamStream = ResponseStream;

        async fn execute_stream(
            &self,
            req: Request<tonic::Streaming<ExecuteCommandStream>>,
        ) -> Result<Response<Self::ExecuteStreamStream>, Status> {
            let out = req.into_inner().map(|r| {
                r.map(|c| ExecuteResponseStream {
                    response: None,
                    id: c.id,
                })
            });
            Ok(Response::new(Box::pin(out)))
        }
    }

    fn no_body_request() -> ExecuteCommand {
        ExecuteCommand {
            argv: vec![],
            env: vec![],
            timeout_s: None,
        }
    }

    async fn start_streaming_server<W: WorkerStreaming>(
        worker: W,
    ) -> (Channel, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            Server::builder()
                .add_service(WorkerStreamingServer::new(worker))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        let channel = Channel::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        (channel, handle)
    }

    /// A streaming worker response without a `response` body fails the command with an error
    /// naming the request.
    #[tokio::test]
    async fn test_streaming_response_without_body_is_an_error() {
        let (channel, _server) = start_streaming_server(NoBodyStreamingWorker).await;
        let mut client = WorkerClient::stream(channel, WorkerDiagnostics::new())
            .await
            .unwrap();
        let err = client.execute(no_body_request()).await.err().unwrap();
        assert!(
            format!("{err:#}").contains("Worker sent a response without a body for request"),
            "{err:#}"
        );
    }

    async fn timeout_seen_by_worker(ms: u64) -> u64 {
        let seen_timeout_s = Arc::new(AtomicU64::new(12345));
        let (client, _server) = start_server(TimeoutRecordingWorker {
            seen_timeout_s: seen_timeout_s.clone(),
        })
        .await;
        let (handle, _child_alive) = worker_handle(client);
        let (observer, _alive_guard) = LivelinessGuard::create();
        handle
            .exec_cmd(
                &[],
                vec![],
                Some(Duration::from_millis(ms)),
                observer.as_ref(),
            )
            .await;
        seen_timeout_s.load(Ordering::SeqCst)
    }

    /// A timeout with a fractional second is rounded up for the worker, never down to 0.
    #[tokio::test]
    async fn test_subsecond_timeout_is_rounded_up_for_the_worker() {
        assert_eq!(timeout_seen_by_worker(1500).await, 2);
        assert_eq!(timeout_seen_by_worker(500).await, 1);
        assert_eq!(timeout_seen_by_worker(2000).await, 2);
    }

    fn test_handle(client: WorkerClient) -> (WorkerHandle, LivelinessGuard) {
        let diagnostics = match &client {
            WorkerClient::Single(_) => WorkerDiagnostics::new(),
            WorkerClient::Stream { diagnostics, .. } => *diagnostics,
        };
        let (child_exited_observer, child_alive_guard) = LivelinessGuard::create();
        let (_, handle_guard) = LivelinessGuard::create();
        let path = |name: &str| {
            AbsNormPathBuf::from(format!("/tmp/buck2_worker_test/{name}")).expect("absolute path")
        };
        let h = WorkerHandle::new(
            client,
            child_exited_observer,
            StdRedirectPaths {
                stdout: path("stdout"),
                stderr: path("stderr"),
            },
            handle_guard,
            diagnostics,
        );
        (h, child_alive_guard)
    }

    #[derive(Clone, Default)]
    struct WorkerReceiptRecorder {
        events: Arc<Mutex<Vec<BuckMutMap<String, String>>>>,
        response_warnings: Arc<AtomicU32>,
        diagnostics_disabled: bool,
    }

    struct WorkerReceiptFields(BuckMutMap<String, String>);

    impl Visit for WorkerReceiptFields {
        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            self.0.insert(field.name().to_owned(), format!("{value:?}"));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_owned(), value.to_owned());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().to_owned(), value.to_string());
        }
    }

    impl Subscriber for WorkerReceiptRecorder {
        fn enabled(&self, metadata: &Metadata<'_>) -> bool {
            metadata.target() != "buck2_worker_diagnostic" || !self.diagnostics_disabled
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            if event.metadata().target() == "buck2_worker_diagnostic" {
                let mut fields = WorkerReceiptFields(Default::default());
                event.record(&mut fields);
                self.events.lock().unwrap().push(fields.0);
            } else if event.metadata().target().ends_with("executors::worker")
                && event.metadata().fields().field("error").is_some()
            {
                // Count the preserved warning without visiting its freeform error field.
                self.response_warnings.fetch_add(1, Ordering::Relaxed);
            }
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[tokio::test]
    async fn test_worker_handle_drop_receipt_is_payload_free() {
        let recorder = WorkerReceiptRecorder::default();
        tracing::subscriber::with_default(recorder.clone(), || {
            let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
            let (handle, _child_alive) = test_handle(WorkerClient::single(channel));
            drop(handle);
        });
        let events = recorder.events.lock().unwrap();
        assert_eq!(events.len(), 1, "missing worker handle-drop observation");
        assert_eq!(events[0]["event"], "handle-drop-start");
        assert!(events[0].contains_key("worker_ordinal"));
        assert!(
            events[0]
                .keys()
                .all(|key| matches!(key.as_str(), "message" | "worker_ordinal" | "event"))
        );
    }

    #[test]
    fn test_worker_reader_preserves_duplicate_errors_and_eof_observations() {
        let recorder = WorkerReceiptRecorder::default();
        let diagnostics = WorkerDiagnostics::new();
        let waiters = WorkerResponseWaiters::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        waiters.insert(7, tx);
        let (observer, guard) = LivelinessGuard::create_sync();
        tracing::subscriber::with_default(recorder.clone(), || {
            let mut reader = WorkerResponseReader::new(diagnostics);
            reader.response(Err(Status::unknown("SYNTHETIC_PRIVATE_PAYLOAD")), &waiters);
            reader.response(Err(Status::unknown("SYNTHETIC_PRIVATE_PAYLOAD")), &waiters);
            reader.eof();
            assert!(
                observer.is_alive_sync(),
                "receipt must not drop stream liveness"
            );
            assert_eq!(
                waiters.len(),
                1,
                "error observations must not remove waiters"
            );
            assert!(!waiters.get(&7).unwrap().is_closed());
        });
        drop(guard);
        assert!(!observer.is_alive_sync());
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert_eq!(recorder.response_warnings.load(Ordering::Relaxed), 2);
        let events = recorder.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["reader_sequence"], "1");
        assert_eq!(events[1]["reader_sequence"], "2");
        assert_eq!(events[2]["reader_sequence"], "2");
        assert_eq!(events[2]["event"], "response-stream-eof-observed");
        for event in &events[..2] {
            assert_eq!(event["worker_ordinal"], diagnostics.ordinal.to_string());
            assert_eq!(
                event["tonic_code"],
                (tonic::Code::Unknown as i32).to_string()
            );
            assert_eq!(event["registered_entries_observed"], "1");
            assert_eq!(event["open_receiver_entries_observed"], "1");
            assert!(
                !event.contains_key("request_id"),
                "a stream error has no request id"
            );
            assert!(event.keys().all(|key| matches!(
                key.as_str(),
                "message"
                    | "worker_ordinal"
                    | "event"
                    | "reader_sequence"
                    | "tonic_code"
                    | "registered_entries_observed"
                    | "open_receiver_entries_observed"
            )));
        }
        assert!(
            events
                .iter()
                .flat_map(|event| event.values())
                .all(|value| !value.contains("SYNTHETIC_PRIVATE_PAYLOAD"))
        );
    }

    #[test]
    fn test_worker_reader_disabled_diagnostics_preserve_errors_and_waiters() {
        let recorder = WorkerReceiptRecorder {
            diagnostics_disabled: true,
            ..Default::default()
        };
        let waiters = WorkerResponseWaiters::default();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        waiters.insert(7, tx);
        tracing::subscriber::with_default(recorder.clone(), || {
            let mut reader = WorkerResponseReader::new(WorkerDiagnostics::new());
            reader.response(Err(Status::unknown("SYNTHETIC_PRIVATE_PAYLOAD")), &waiters);
            reader.response(Err(Status::unknown("SYNTHETIC_PRIVATE_PAYLOAD")), &waiters);
            reader.eof();
            assert_eq!(reader.sequence, 2);
            assert_eq!(waiters.len(), 1);
            assert!(!waiters.get(&7).unwrap().is_closed());
        });
        assert!(recorder.events.lock().unwrap().is_empty());
        assert_eq!(recorder.response_warnings.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_worker_reader_response_receipt_is_payload_free_and_delivery_unchanged() {
        let recorder = WorkerReceiptRecorder::default();
        let diagnostics = WorkerDiagnostics::new();
        let waiters = WorkerResponseWaiters::default();
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        waiters.insert(7, tx);
        tracing::subscriber::with_default(recorder.clone(), || {
            let mut reader = WorkerResponseReader::new(diagnostics);
            reader.response(
                Ok(ExecuteResponseStream {
                    id: 7,
                    response: Some(ExecuteResponse {
                        exit_code: 5,
                        stderr: "SYNTHETIC_PRIVATE_PAYLOAD".to_owned(),
                        timed_out_after_s: None,
                    }),
                }),
                &waiters,
            );
            reader.eof();
        });
        let response = rx.try_recv().unwrap().response.unwrap();
        assert_eq!(response.exit_code, 5);
        assert_eq!(response.stderr, "SYNTHETIC_PRIVATE_PAYLOAD");
        assert!(waiters.is_empty());
        let events = recorder.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        let event = &events[0];
        assert_eq!(event["event"], "response-item-observed");
        assert_eq!(event["worker_ordinal"], diagnostics.ordinal.to_string());
        assert_eq!(event["request_id"], "7");
        assert_eq!(event["reader_sequence"], "1");
        assert!(!event.contains_key("registered_entries_observed"));
        assert!(!event.contains_key("open_receiver_entries_observed"));
        assert!(event.keys().all(|key| matches!(
            key.as_str(),
            "message" | "worker_ordinal" | "event" | "request_id" | "reader_sequence"
        )));
        assert!(
            event
                .values()
                .all(|value| !value.contains("SYNTHETIC_PRIVATE_PAYLOAD"))
        );
    }

    #[tokio::test]
    async fn test_worker_handle_receipt_preserves_arc_and_owner_guard_lifetime() {
        let recorder = WorkerReceiptRecorder::default();
        tracing::subscriber::with_default(recorder.clone(), || {
            let channel = Channel::from_static("http://127.0.0.1:1").connect_lazy();
            let (child_observer, _child_guard) = LivelinessGuard::create();
            let (owner_observer, owner_guard) = LivelinessGuard::create_sync();
            let path = AbsNormPathBuf::from("/tmp/buck2_worker_test/unused".to_owned()).unwrap();
            let handle = Arc::new(WorkerHandle::new(
                WorkerClient::single(channel),
                child_observer,
                StdRedirectPaths {
                    stdout: path.clone(),
                    stderr: path,
                },
                owner_guard,
                WorkerDiagnostics::new(),
            ));
            let clone = handle.dupe();
            drop(handle);
            assert!(owner_observer.is_alive_sync());
            assert!(recorder.events.lock().unwrap().is_empty());
            drop(clone);
            assert!(!owner_observer.is_alive_sync());
        });
        let events = recorder.events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "handle-drop-start");
    }

    struct WorkerStreamFixture {
        client: WorkerClient,
        commands: tokio::sync::mpsc::UnboundedReceiver<ExecuteCommandStream>,
        waiters: Arc<WorkerResponseWaiters>,
        stream_guard: Option<LivelinessGuard>,
    }

    impl WorkerStreamFixture {
        fn new() -> Self {
            let (stream, commands) = tokio::sync::mpsc::unbounded_channel();
            let (stream_closed_observer, stream_guard) = LivelinessGuard::create();
            let waiters: Arc<WorkerResponseWaiters> = Default::default();
            Self {
                client: WorkerClient::Stream {
                    ids: Default::default(),
                    stream,
                    stream_closed_observer,
                    waiters: waiters.dupe(),
                    diagnostics: WorkerDiagnostics::new(),
                },
                commands,
                waiters,
                stream_guard: Some(stream_guard),
            }
        }
    }

    #[test]
    fn test_worker_request_receipts_preserve_all_closed_outcomes() {
        for outcome in [
            "send-failed",
            "response-returned",
            "response-body-missing",
            "receive-closed",
            "stream-closed",
            "future-dropped-before-return",
        ] {
            let recorder = WorkerReceiptRecorder::default();
            tracing::subscriber::with_default(recorder.clone(), || {
                let mut fixture = WorkerStreamFixture::new();
                if outcome == "send-failed" {
                    fixture.commands.close();
                }
                let mut future = Box::pin(fixture.client.execute(no_body_request()));
                let mut context = Context::from_waker(noop_waker_ref());
                let first = future.as_mut().poll(&mut context);
                if outcome == "send-failed" {
                    assert!(matches!(first, Poll::Ready(Err(_))));
                    return;
                }
                assert!(first.is_pending());
                let command = fixture.commands.try_recv().unwrap();
                assert_eq!(command.id, 0);
                match outcome {
                    "response-returned" | "response-body-missing" => {
                        let response = (outcome == "response-returned").then(|| ExecuteResponse {
                            exit_code: 7,
                            stderr: "SYNTHETIC_PRIVATE_PAYLOAD".to_owned(),
                            timed_out_after_s: None,
                        });
                        fixture
                            .waiters
                            .remove(&0)
                            .unwrap()
                            .1
                            .send(ExecuteResponseStream { response, id: 0 })
                            .unwrap();
                        if outcome == "response-returned" {
                            // Both arms ready: the unchanged bias must still choose the response.
                            drop(fixture.stream_guard.take());
                        }
                    }
                    "receive-closed" => {
                        fixture.waiters.remove(&0);
                    }
                    "stream-closed" => {
                        drop(fixture.stream_guard.take());
                    }
                    "future-dropped-before-return" => {
                        drop(future);
                        assert_eq!(fixture.waiters.len(), 1);
                        assert!(fixture.waiters.get(&0).unwrap().is_closed());
                        return;
                    }
                    _ => unreachable!(),
                }
                let result = match future.as_mut().poll(&mut context) {
                    Poll::Ready(result) => result,
                    Poll::Pending => panic!("terminal control did not complete request"),
                };
                if outcome == "response-returned" {
                    let response = result.unwrap();
                    assert_eq!(response.exit_code, 7);
                    assert_eq!(response.stderr, "SYNTHETIC_PRIVATE_PAYLOAD");
                } else {
                    assert!(
                        result.is_err(),
                        "diagnostics must not turn failure into success"
                    );
                }
            });
            let events = recorder.events.lock().unwrap();
            let ended = events
                .iter()
                .find(|event| event["event"] == "request-wait-ended")
                .unwrap();
            assert_eq!(ended["outcome"], outcome);
            assert_eq!(ended["request_id"], "0");
            assert!(ended.keys().all(|key| matches!(
                key.as_str(),
                "message" | "worker_ordinal" | "event" | "request_id" | "outcome"
            )));
            assert!(
                events
                    .iter()
                    .flat_map(|event| event.values())
                    .all(|value| !value.contains("SYNTHETIC_PRIVATE_PAYLOAD"))
            );
        }
    }

    #[test]
    fn test_worker_cancelled_receiver_observations_do_not_clean_waiters() {
        let recorder = WorkerReceiptRecorder::default();
        tracing::subscriber::with_default(recorder.clone(), || {
            let mut fixture = WorkerStreamFixture::new();
            let diagnostics = match &fixture.client {
                WorkerClient::Stream { diagnostics, .. } => *diagnostics,
                _ => unreachable!(),
            };
            let mut future = Box::pin(fixture.client.execute(no_body_request()));
            let mut context = Context::from_waker(noop_waker_ref());
            assert!(future.as_mut().poll(&mut context).is_pending());
            drop(future);
            let (tx, _live_receiver) = tokio::sync::oneshot::channel();
            fixture.waiters.insert(99, tx);
            let mut reader = WorkerResponseReader::new(diagnostics);
            reader.response(
                Err(Status::unknown("SYNTHETIC_PRIVATE_PAYLOAD")),
                &fixture.waiters,
            );
            assert_eq!(
                fixture.waiters.len(),
                2,
                "no cancellation cleanup in diagnostics"
            );
        });
        let events = recorder.events.lock().unwrap();
        let error = events
            .iter()
            .find(|event| event["event"] == "response-error-observed")
            .unwrap();
        assert_eq!(error["registered_entries_observed"], "2");
        assert_eq!(error["open_receiver_entries_observed"], "1");
    }

    #[test]
    fn test_worker_clones_share_ordinal_and_instances_distinguish_request_zero() {
        let recorder = WorkerReceiptRecorder::default();
        tracing::subscriber::with_default(recorder.clone(), || {
            let mut first = WorkerStreamFixture::new();
            let mut cloned = first.client.clone();
            let mut second = WorkerStreamFixture::new();
            let mut context = Context::from_waker(noop_waker_ref());
            for client in [&mut first.client, &mut cloned, &mut second.client] {
                let mut future = Box::pin(client.execute(no_body_request()));
                assert!(future.as_mut().poll(&mut context).is_pending());
            }
        });
        let events = recorder.events.lock().unwrap();
        let registered: Vec<_> = events
            .iter()
            .filter(|event| event["event"] == "request-wait-registered")
            .collect();
        assert_eq!(registered.len(), 3);
        assert_eq!(
            registered[0]["worker_ordinal"],
            registered[1]["worker_ordinal"]
        );
        assert_ne!(
            registered[0]["worker_ordinal"],
            registered[2]["worker_ordinal"]
        );
        assert_eq!(registered[0]["request_id"], "0");
        assert_eq!(registered[1]["request_id"], "1");
        assert_eq!(registered[2]["request_id"], "0");
    }

    #[test]
    fn test_worker_exit_receipts_exclude_payloads_and_preserve_results() {
        let recorder = WorkerReceiptRecorder::default();
        let diagnostics = WorkerDiagnostics::new();
        let results = [
            Ok(GatherOutputStatus::Finished {
                exit_code: 7,
                execution_stats: None,
            }),
            Ok(GatherOutputStatus::SpawnFailed(
                "SYNTHETIC_PRIVATE_PAYLOAD".to_owned(),
            )),
            Ok(GatherOutputStatus::Cancelled),
            Ok(GatherOutputStatus::TimedOut(Duration::from_secs(3))),
            Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "SYNTHETIC_PRIVATE_PAYLOAD"
            )),
        ];
        tracing::subscriber::with_default(recorder.clone(), || {
            for result in &results {
                diagnostics.exit_observed(result);
            }
        });
        assert!(matches!(
            &results[0],
            Ok(GatherOutputStatus::Finished { exit_code: 7, .. })
        ));
        assert!(results[4].is_err());
        let events = recorder.events.lock().unwrap();
        let kinds: Vec<_> = events
            .iter()
            .map(|event| event["exit_kind"].as_str())
            .collect();
        assert_eq!(
            kinds,
            [
                "finished",
                "spawn-failed",
                "cancelled",
                "timed-out",
                "observation-error"
            ]
        );
        assert_eq!(events[0]["exit_code"], "7");
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event.contains_key("exit_code"), index == 0);
            assert!(event.keys().all(|key| matches!(
                key.as_str(),
                "message" | "worker_ordinal" | "event" | "exit_kind" | "exit_code"
            )));
            assert!(
                event
                    .values()
                    .all(|value| !value.contains("SYNTHETIC_PRIVATE_PAYLOAD"))
            );
        }
    }

    /// A streaming worker that answers each request and then ends its response stream, as when
    /// the worker process exits right after writing its last answer.
    struct AnswerThenCloseWorker;

    #[tonic::async_trait]
    impl WorkerStreaming for AnswerThenCloseWorker {
        type ExecuteStreamStream = ResponseStream;

        async fn execute_stream(
            &self,
            req: Request<tonic::Streaming<ExecuteCommandStream>>,
        ) -> Result<Response<Self::ExecuteStreamStream>, Status> {
            let mut inbound = req.into_inner();
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            tokio::spawn(async move {
                if let Some(Ok(cmd)) = inbound.next().await {
                    let _unused = tx.send(Ok(ExecuteResponseStream {
                        response: Some(ExecuteResponse {
                            exit_code: 7,
                            stderr: "answer".to_owned(),
                            timed_out_after_s: None,
                        }),
                        id: cmd.id,
                    }));
                }
                // Returning drops `tx`: the response stream ends right after the answer.
            });
            Ok(Response::new(Box::pin(
                tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
            )))
        }
    }

    /// Runs 40 commands against workers that answer and then close, and counts how many answers
    /// came back and how many were reported as a closed stream.
    async fn answered_and_lost() -> (usize, usize) {
        let mut answered = 0;
        let mut lost = 0;
        for _ in 0..40 {
            let (channel, _server) = start_streaming_server(AnswerThenCloseWorker).await;
            let client = WorkerClient::stream(channel, WorkerDiagnostics::new())
                .await
                .unwrap();
            let (h, _child_alive) = test_handle(client);
            let (observer, _alive) = LivelinessGuard::create();
            let r = h.exec_cmd(&[], vec![], None, observer.as_ref()).await;
            match r.status {
                GatherOutputStatus::Finished { exit_code: 7, .. } => {
                    assert_eq!(r.stderr, b"answer");
                    answered += 1;
                }
                GatherOutputStatus::SpawnFailed(msg) => {
                    assert!(
                        msg.contains("Stream closed while waiting for response"),
                        "{msg}"
                    );
                    lost += 1;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        (answered, lost)
    }

    /// An answer that arrives together with the end of the stream is returned.
    #[tokio::test(flavor = "current_thread")]
    async fn test_streaming_answer_kept_when_stream_closes_right_after() {
        let (answered, lost) = answered_and_lost().await;
        assert_eq!((answered, lost), (40, 0));
    }
}
