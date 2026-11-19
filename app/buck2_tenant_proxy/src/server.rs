/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The proxy process's side of the daemon API: the methods a client of any release uses on a
//! daemon's directory, answered for the shared daemon.

use std::convert::Infallible;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use buck2_cli_proto::ClientContext;
use buck2_cli_proto::CommandResult;
use buck2_cli_proto::DaemonProcessInfo;
use buck2_cli_proto::EvictProjectTenantsRequest;
use buck2_cli_proto::EvictProjectTenantsResponse;
use buck2_cli_proto::EvictTenantRequest;
use buck2_cli_proto::EvictTenantResponse;
use buck2_cli_proto::KillRequest;
use buck2_cli_proto::KillResponse;
use buck2_cli_proto::StatusRequest;
use buck2_cli_proto::StatusResponse;
use buck2_cli_proto::daemon_api_client::DaemonApiClient;
use buck2_client_ctx::daemon::client::connect::BuckAddAuthTokenInterceptor;
use buck2_client_ctx::daemon::client::connect::new_daemon_api_client;
use buck2_common::buckd_connection::BUCK_AUTH_TOKEN_HEADER;
use buck2_common::buckd_connection::ConnectionType;
use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_common::daemon_dir::DaemonDir;
use buck2_fs::paths::file_name::FileName;
use buck2_server::daemon::proxies::proxy_before_evict_gate;
use buck2_util::threads::thread_spawn;
use buck2_wrapper_common::kill;
use buck2_wrapper_common::pid::Pid;
use tonic::Request;
use tonic::Response;
use tonic::Status;
use tonic::codegen::Body;
use tonic::codegen::BoxFuture as GrpcFuture;
use tonic::codegen::InterceptedService;
use tonic::codegen::Service;
use tonic::codegen::StdError;
use tonic::codegen::http;
use tonic::transport::Channel;

use crate::events::UNIDAEMON_EVENTS_FILE;
use crate::process::daemon_exited;
use crate::slot::slot_files;
use crate::slot::slot_record;

type DaemonClient = DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>;

/// How the shared daemon answered a forwarded eviction.
enum Forwarded {
    /// The daemon is evicting the generation; the connection is the one to await it on.
    Evicting(Box<(DaemonClient, ClientContext)>),
    /// The daemon no longer holds the generation, so there is nothing to wait for.
    NothingToEvict,
    /// The daemon could not be reached in time, or refused the request.
    Failed,
}

pub(crate) struct ProxyState {
    pub(crate) spec: TenantProxySpec,
    pub(crate) info: DaemonProcessInfo,
    pub(crate) directory: DaemonDir,
    pub(crate) started: Instant,
    pub(crate) start_time: SystemTime,
    pub(crate) kill_started: AtomicBool,
    pub(crate) force_started: AtomicBool,
    pub(crate) eviction_started: Mutex<Option<Instant>>,
    pub(crate) finished: AtomicBool,
    pub(crate) notify: tokio::sync::Notify,
    pub(crate) monitor: Mutex<UnixStream>,
}

/// How long an eviction request waits for the daemon to accept it: all of `budget` but the second
/// kept for the forced exit, or half of a budget too short to spare a second. The daemon is
/// always asked before it is forced.
fn accept_window(budget: Duration) -> Duration {
    budget.saturating_sub(Duration::from_secs(1).min(budget / 2))
}

impl ProxyState {
    fn authorize<T>(&self, request: &Request<T>) -> Result<(), Status> {
        if request
            .metadata()
            .get(BUCK_AUTH_TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            != Some(self.info.auth_token.as_str())
        {
            return Err(Status::unauthenticated("invalid proxy authentication"));
        }
        Ok(())
    }

    /// Appends one line to the tenant's event log (see `events`). Best effort: the log is
    /// diagnostic, and a proxy that cannot write it must still finish its eviction. The append
    /// is synchronous on the caller's task: one short write to a local file costs less than
    /// handing it to a blocking thread, and must not delay the eviction it describes.
    pub(crate) fn record(&self, event: &str, reason: &str) {
        let path = self
            .directory
            .path
            .parent()
            .expect("legacy slot has a parent")
            .join(FileName::unchecked_new(UNIDAEMON_EVENTS_FILE));
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let elapsed = self
                .eviction_started
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .map(|started| Instant::now() - started)
                .unwrap_or_else(|| Instant::now() - self.started);
            let row = serde_json::json!({"event":event,"reason":reason,"generation":self.spec.reference.generation,"daemon_id":self.spec.reference.daemon_id,"proxy_pid":self.info.pid,"compatibility_key":self.spec.reference.compatibility_key,"elapsed_ms":elapsed.as_millis()});
            if let Ok(mut line) = serde_json::to_vec(&row) {
                line.push(b'\n');
                let _ignored = file.write_all(&line);
            }
        }
    }

    pub(crate) fn finish(&self) {
        self.finished.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    fn force_daemon(&self) {
        if self.finished.load(Ordering::Acquire) || self.force_started.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let Ok(pid) = Pid::from_i64(self.spec.daemon.pid) else {
            self.finish();
            return;
        };
        // The tree kill leaves a process whose recorded start time is not the daemon's alone,
        // with the tolerance every reader of a recorded start time applies. A released client
        // kills this proxy four seconds after sending Kill, and this runs at three seconds, so
        // the tree kill has to finish, killing whatever it stopped, before then.
        let confirmed = kill::kill_unidaemon_tree_within(
            pid,
            self.spec.daemon.pid_start_time,
            Duration::from_millis(400),
        );
        self.finish();
        self.record(
            if confirmed {
                "forced-daemon-exit"
            } else {
                "forced-daemon-exit-unconfirmed"
            },
            "eviction-timeout-or-rpc-failure",
        );
    }

    async fn status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        self.authorize(&request)?;
        let response = StatusResponse {
            process_info: Some(self.info.clone()),
            daemon_constraints: Some(self.spec.constraints.clone()),
            project_root: self.spec.tenant.project_root.clone(),
            isolation_dir: self.spec.tenant.isolation.clone(),
            start_time: Some(self.start_time.into()),
            uptime: Some(
                (Instant::now() - self.started)
                    .try_into()
                    .map_err(|_| Status::internal("invalid uptime"))?,
            ),
            valid_working_directory: Some(Path::new(&self.spec.tenant.project_root).is_dir()),
            valid_buck_out_mount: Some(true),
            io_provider: self.spec.io_provider.clone(),
            tenant_generation: Some(self.spec.reference.generation),
            ..Default::default()
        };
        Ok(Response::new(CommandResult {
            result: Some(response.into()),
        }))
    }

    async fn kill(
        self: &Arc<Self>,
        request: Request<KillRequest>,
    ) -> Result<Response<CommandResult>, Status> {
        self.authorize(&request)?;
        let request = request.into_inner();
        if !self.kill_started.swap(true, Ordering::AcqRel) {
            let budget = self.arm_eviction(&request)?;
            self.wait_for_before_evict_gate().await?;
            match self.forward_eviction(&request, budget).await {
                Forwarded::Evicting(connection) => {
                    let (client, context) = *connection;
                    self.await_eviction(client, context);
                }
                Forwarded::NothingToEvict => self.finish(),
                Forwarded::Failed => self.force_daemon_in_background(),
            }
        }
        Ok(Response::new(CommandResult {
            result: Some(KillResponse {}.into()),
        }))
    }

    /// Starts the eviction clock: computes the budget the shared daemon gets, arms the watchdog
    /// that forces the daemon's exit when the budget runs out, and tells the daemon over the
    /// liveness socket that a kill is in progress. Returns the budget.
    fn arm_eviction(self: &Arc<Self>, request: &KillRequest) -> Result<Duration, Status> {
        *self
            .eviction_started
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Instant::now());
        let timeout = request
            .timeout
            .and_then(|value| Duration::try_from(value).ok())
            .unwrap_or(Duration::from_secs(4));
        // The client kills this proxy when its timeout runs out, counted from its own send: a
        // released client's four seconds, or the longer grace a client of this release sends
        // and waits for. Leave a second for the forced tree kill and for the delay before this
        // request arrived, but always ask the daemon before forcing it.
        let budget = timeout
            .saturating_sub(Duration::from_secs(1))
            .max(Duration::from_secs(1));
        let watch = self.clone();
        thread_spawn("buck2-proxy-eviction-watchdog", move || {
            std::thread::sleep(budget);
            if !watch.finished.load(Ordering::Acquire) {
                watch.force_daemon();
            }
        })
        .map_err(|error| Status::internal(format!("{error:#}")))?;
        self.record("eviction-start", &request.reason);
        let _ignored = self
            .monitor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .write_all(b"K");
        Ok(budget)
    }

    /// Holds the eviction at the `BUCK2_TEST_PROXY_BEFORE_EVICT` gate when a test set one for
    /// this tenant.
    async fn wait_for_before_evict_gate(&self) -> Result<(), Status> {
        let Some(gate) =
            proxy_before_evict_gate().map_err(|error| Status::internal(format!("{error:#}")))?
        else {
            return Ok(());
        };
        let Some((isolation, path)) = gate.split_once(',') else {
            return Ok(());
        };
        if isolation != self.spec.tenant.isolation {
            return Ok(());
        }
        tokio::fs::write(format!("{path}.started"), b"before-forward")
            .await
            .map_err(|error| Status::internal(error.to_string()))?;
        let release = format!("{path}.release");
        while !tokio::fs::try_exists(&release).await.unwrap_or(false) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    /// Asks the shared daemon to evict this proxy's generation within `budget`, without waiting
    /// for the eviction to complete. The request itself gets the budget less the second kept
    /// for the forced exit, so that a daemon that does not answer still leaves time for it.
    async fn forward_eviction(
        self: &Arc<Self>,
        request: &KillRequest,
        budget: Duration,
    ) -> Forwarded {
        let context = ClientContext {
            working_dir: self.spec.tenant.project_root.clone(),
            tenant_identity: Some(self.spec.tenant.clone()),
            command_name: if request.reason.contains("clean") {
                "clean"
            } else {
                "kill"
            }
            .to_owned(),
            ..Default::default()
        };
        let begin = async {
            let endpoint = ConnectionType::parse(&self.spec.daemon.endpoint)?;
            let mut client =
                new_daemon_api_client(endpoint, self.spec.daemon.auth_token.clone()).await?;
            let response = client
                .evict_tenant(Request::new(EvictTenantRequest {
                    context: Some(context.clone()),
                    expected_generation: Some(self.spec.reference.generation),
                    eviction_timeout_ms: Some(budget.as_millis() as u64),
                    wait_for_completion: Some(false),
                }))
                .await?
                .into_inner();
            Ok::<_, buck2_error::Error>((client, response))
        };
        match tokio::time::timeout(accept_window(budget), begin).await {
            Ok(Ok((client, response))) => {
                self.record("daemon-switch-armed", &request.reason);
                if response.evicted {
                    Forwarded::Evicting(Box::new((client, context)))
                } else {
                    Forwarded::NothingToEvict
                }
            }
            _ => Forwarded::Failed,
        }
    }

    /// Waits, off the request's task, for the eviction the daemon accepted to complete, and for
    /// the daemon to exit when the generation was its last tenant; a daemon that stops answering
    /// before then is forced out.
    fn await_eviction(self: &Arc<Self>, mut client: DaemonClient, context: ClientContext) {
        let state = self.clone();
        tokio::spawn(async move {
            let result = client
                .evict_tenant(Request::new(EvictTenantRequest {
                    context: Some(context),
                    expected_generation: Some(state.spec.reference.generation),
                    wait_for_completion: Some(true),
                    ..Default::default()
                }))
                .await;
            match result {
                Ok(response) => {
                    if response.get_ref().shutting_down {
                        while !daemon_exited(state.spec.daemon.pid) {
                            tokio::time::sleep(Duration::from_millis(2)).await;
                        }
                    }
                    state.finish();
                    state.record("eviction-complete", "graceful");
                }
                Err(_) if daemon_exited(state.spec.daemon.pid) => state.finish(),
                Err(_) => state.force_daemon_in_background(),
            }
        });
    }

    /// Forces the daemon's exit on a thread of its own: the tree kill blocks, and the watchdog
    /// may be running it at the same time, which `force_daemon` tolerates.
    fn force_daemon_in_background(self: &Arc<Self>) {
        let state = self.clone();
        let _ignored = thread_spawn("buck2-proxy-fallback", move || state.force_daemon());
    }

    async fn evict(
        self: &Arc<Self>,
        request: Request<EvictTenantRequest>,
    ) -> Result<Response<EvictTenantResponse>, Status> {
        self.authorize(&request)?;
        let matches = request
            .get_ref()
            .expected_generation
            .is_none_or(|generation| generation == self.spec.reference.generation)
            && request
                .get_ref()
                .context
                .as_ref()
                .and_then(|context| context.tenant_identity.as_ref())
                .is_none_or(|identity| identity == &self.spec.tenant);
        if !matches {
            return Ok(Response::new(EvictTenantResponse::default()));
        }
        let wait = request.get_ref().wait_for_completion.unwrap_or(true);
        let reason = request
            .get_ref()
            .context
            .as_ref()
            .map(|context| context.command_name.as_str())
            .unwrap_or("kill");
        let mut kill = Request::new(KillRequest {
            reason: format!("Tenant eviction requested by `{reason}`"),
            timeout: Some(
                Duration::from_secs(4)
                    .try_into()
                    .map_err(|_| Status::internal("invalid timeout"))?,
            ),
            ..Default::default()
        });
        *kill.metadata_mut() = request.metadata().clone();
        self.kill(kill).await?;
        if wait {
            while !self.finished.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
        Ok(Response::new(EvictTenantResponse {
            evicted: true,
            generation: self.spec.reference.generation,
            shutting_down: false,
        }))
    }

    async fn evict_project(
        self: &Arc<Self>,
        request: Request<EvictProjectTenantsRequest>,
    ) -> Result<Response<EvictProjectTenantsResponse>, Status> {
        self.authorize(&request)?;
        if request
            .get_ref()
            .isolation
            .as_ref()
            .is_some_and(|isolation| isolation != &self.spec.tenant.isolation)
        {
            return Ok(Response::new(EvictProjectTenantsResponse::default()));
        }
        let mut evict = Request::new(EvictTenantRequest {
            context: request.get_ref().context.clone(),
            ..Default::default()
        });
        *evict.metadata_mut() = request.metadata().clone();
        let response = self.evict(evict).await?.into_inner();
        Ok(Response::new(EvictProjectTenantsResponse {
            generations: if response.evicted {
                vec![response.generation]
            } else {
                Vec::new()
            },
            shutting_down: false,
        }))
    }

    /// Removes the slot files if the slot still names this process: a successor that took the
    /// slot keeps its files.
    pub(crate) fn cleanup(&self) {
        let still_owner = slot_record(&self.directory).is_some_and(|info| {
            info.pid == self.info.pid && info.pid_start_time == self.info.pid_start_time
        });
        if !still_owner {
            return;
        }
        for path in slot_files(&self.directory) {
            let _ignored = std::fs::remove_file(path);
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProxyService(pub(crate) Arc<ProxyState>);

struct KillService(Arc<ProxyState>);
impl tonic::server::UnaryService<KillRequest> for KillService {
    type Response = CommandResult;
    type Future = GrpcFuture<Response<CommandResult>, Status>;
    fn call(&mut self, request: Request<KillRequest>) -> Self::Future {
        let state = self.0.clone();
        Box::pin(async move { state.kill(request).await })
    }
}
struct StatusService(Arc<ProxyState>);
impl tonic::server::UnaryService<StatusRequest> for StatusService {
    type Response = CommandResult;
    type Future = GrpcFuture<Response<CommandResult>, Status>;
    fn call(&mut self, request: Request<StatusRequest>) -> Self::Future {
        let state = self.0.clone();
        Box::pin(async move { state.status(request).await })
    }
}
struct EvictService(Arc<ProxyState>);
impl tonic::server::UnaryService<EvictTenantRequest> for EvictService {
    type Response = EvictTenantResponse;
    type Future = GrpcFuture<Response<Self::Response>, Status>;
    fn call(&mut self, request: Request<EvictTenantRequest>) -> Self::Future {
        let state = self.0.clone();
        Box::pin(async move { state.evict(request).await })
    }
}
struct EvictProjectService(Arc<ProxyState>);
impl tonic::server::UnaryService<EvictProjectTenantsRequest> for EvictProjectService {
    type Response = EvictProjectTenantsResponse;
    type Future = GrpcFuture<Response<Self::Response>, Status>;
    fn call(&mut self, request: Request<EvictProjectTenantsRequest>) -> Self::Future {
        let state = self.0.clone();
        Box::pin(async move { state.evict_project(request).await })
    }
}
impl<B> Service<http::Request<B>> for ProxyService
where
    B: Body + Send + 'static,
    B::Error: Into<StdError> + Send + 'static,
{
    type Response = http::Response<tonic::body::Body>;
    type Error = Infallible;
    type Future = GrpcFuture<Self::Response, Self::Error>;
    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        let state = self.0.clone();
        match request.uri().path() {
            "/buck.daemon.DaemonApi/Kill" => Box::pin(async move {
                Ok(tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
                    .unary(KillService(state), request)
                    .await)
            }),
            "/buck.daemon.DaemonApi/Status" => Box::pin(async move {
                Ok(tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
                    .unary(StatusService(state), request)
                    .await)
            }),
            "/buck.daemon.DaemonApi/EvictTenant" => Box::pin(async move {
                Ok(tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
                    .unary(EvictService(state), request)
                    .await)
            }),
            "/buck.daemon.DaemonApi/EvictProjectTenants" => Box::pin(async move {
                Ok(tonic::server::Grpc::new(tonic_prost::ProstCodec::default())
                    .unary(EvictProjectService(state), request)
                    .await)
            }),
            _ => Box::pin(async {
                Ok(http::Response::builder()
                    .status(200)
                    .header("grpc-status", "12")
                    .header("content-type", "application/grpc")
                    .body(tonic::body::Body::empty())
                    .expect("constant response"))
            }),
        }
    }
}
impl tonic::server::NamedService for ProxyService {
    const NAME: &'static str = "buck.daemon.DaemonApi";
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::server::accept_window;

    #[test]
    fn accept_window_leaves_time_to_force_without_skipping_the_daemon() {
        assert_eq!(
            accept_window(Duration::from_secs(10)),
            Duration::from_secs(9)
        );
        assert_eq!(
            accept_window(Duration::from_secs(3)),
            Duration::from_secs(2)
        );
        assert_eq!(
            accept_window(Duration::from_secs(1)),
            Duration::from_millis(500)
        );
    }
}
