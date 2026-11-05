/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::borrow::Cow;
use std::env;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fmt;
use std::fmt::Display;
use std::fs::File;
use std::io::BufReader;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use buck2_cli_proto::DaemonProcessInfo;
use buck2_cli_proto::daemon_api_client::DaemonApiClient;
use buck2_common::buckd_connection::BUCK_AUTH_TOKEN_HEADER;
use buck2_common::buckd_connection::ConnectionType;
use buck2_common::client_utils::RetryError;
use buck2_common::client_utils::get_channel_tcp;
use buck2_common::client_utils::get_channel_uds;
use buck2_common::client_utils::retrying;
use buck2_common::daemon_dir::DaemonDir;
use buck2_common::init::DaemonStartupConfig;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_common::invocation_paths::TenantDiskLock;
use buck2_core::buck2_env;
use buck2_core::soft_error_or_panic;
use buck2_data::DaemonWasStartedReason;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_error::conversion::from_any_with_tag;
use buck2_events::daemon_id::DaemonId;
use buck2_fs::error::IoResultExt;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_resource_control::spawn_daemon::create_daemon_spawn_command;
use buck2_settings::BuckSettingChange;
use buck2_util::truncate::truncate;
use buck2_wrapper_common::invocation_id::TraceId;
use buck2_wrapper_common::kill::process_exists;
use buck2_wrapper_common::pid::Pid;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::try_join3;
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::time::timeout;
use tonic::Request;
use tonic::Status;
use tonic::codegen::InterceptedService;
use tonic::metadata::AsciiMetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;

use crate::client_ctx::AgentAdvice;
use crate::command_outcome::CommandOutcome;
use crate::daemon::client::BuckdClient;
use crate::daemon::client::BuckdClientConnector;
use crate::daemon::client::BuckdLifecycleLock;
use crate::daemon::client::kill;
use crate::daemon::client::kill::hard_kill_until;
use crate::daemon::daemon_windows::spawn_background_process_on_windows;
use crate::daemon_constraints;
use crate::daemon_constraints::get_possibly_nested_invocation_daemon_uuid;
use crate::events_ctx::DaemonEventsCtx;
use crate::events_ctx::EventsCtx;
use crate::immediate_config::ImmediateConfigContext;
use crate::startup_deadline::StartupDeadline;
use crate::subscribers::classify_server_stderr::classify_server_stderr;
use crate::subscribers::stdout_stderr_forwarder::StdoutStderrForwarder;
use crate::subscribers::system_warning::is_disk_near_full;

#[cfg(all(fbcode_build, target_os = "linux"))]
mod linux_unsandbox;

use futures::future::LocalBoxFuture;

#[cfg(all(fbcode_build, target_os = "linux"))]
use self::linux_unsandbox::get_unix_daemon_and_args;

#[cfg(fbcode_build)]
const HTTP_PROXY_ENV_VARS: [&str; 6] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
];

/// The client side matcher for DaemonConstraints.
#[derive(Clone, Debug)]
pub struct DaemonConstraintsRequest {
    /// The version of buck2.
    version: String,
    /// Sandcastle id.
    user_version: Option<String>,
    desired_trace_io_state: DesiredTraceIoState,
    nested_invocation_daemon_uuid: Option<String>,
    pub reject_daemon: Option<String>,
    pub reject_materializer_state: Option<String>,
    pub daemon_startup_config: DaemonStartupConfig,
}

#[derive(Debug, derive_more::Display)]
pub(crate) enum ConstraintUnsatisfiedReason {
    #[display("Version mismatch")]
    Version,
    #[display("User version mismatch")]
    UserVersion,
    #[display("Startup config mismatch")]
    StartupConfig,
    #[display("Buck settings changed ({})", changes.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    BuckSettings { changes: Vec<BuckSettingChange> },
    #[display("Reject daemon id")]
    RejectDaemonId,
    #[display("Trace IO mismatch")]
    TraceIo,
    #[display("Sqlite identity mismatch")]
    SqliteIdentity,
}

impl ConstraintUnsatisfiedReason {
    pub(crate) fn to_daemon_was_started_reason(&self) -> buck2_data::DaemonWasStartedReason {
        match self {
            ConstraintUnsatisfiedReason::Version => {
                buck2_data::DaemonWasStartedReason::ConstraintMismatchVersion
            }
            ConstraintUnsatisfiedReason::UserVersion => {
                buck2_data::DaemonWasStartedReason::ConstraintMismatchUserVersion
            }
            ConstraintUnsatisfiedReason::StartupConfig
            | ConstraintUnsatisfiedReason::BuckSettings { .. } => {
                buck2_data::DaemonWasStartedReason::ConstraintMismatchStartupConfig
            }
            ConstraintUnsatisfiedReason::RejectDaemonId => {
                buck2_data::DaemonWasStartedReason::ConstraintRejectDaemonId
            }
            ConstraintUnsatisfiedReason::TraceIo => {
                buck2_data::DaemonWasStartedReason::ConstraintMismatchTraceIo
            }
            ConstraintUnsatisfiedReason::SqliteIdentity => {
                buck2_data::DaemonWasStartedReason::ConstraintMismatchSqliteIdentity
            }
        }
    }
}

impl DaemonConstraintsRequest {
    pub fn new(
        immediate_config: &ImmediateConfigContext<'_>,
        desired_trace_io_state: DesiredTraceIoState,
    ) -> buck2_error::Result<Self> {
        Ok(Self {
            version: daemon_constraints::version()?,
            user_version: daemon_constraints::user_version()?,
            desired_trace_io_state,
            nested_invocation_daemon_uuid: get_possibly_nested_invocation_daemon_uuid(),
            reject_daemon: None,
            reject_materializer_state: None,
            daemon_startup_config: immediate_config.daemon_startup_config()?.clone(),
        })
    }

    fn is_trace_io_requested(&self) -> bool {
        matches!(self.desired_trace_io_state, DesiredTraceIoState::Enabled)
    }

    fn satisfied(
        &self,
        daemon: &buck2_cli_proto::DaemonConstraints,
    ) -> Result<(), ConstraintUnsatisfiedReason> {
        if self.version != daemon.version {
            return Err(ConstraintUnsatisfiedReason::Version);
        }

        if !is_nested_invocation(self.nested_invocation_daemon_uuid.as_ref(), daemon)
            && self.user_version != daemon.user_version
        {
            return Err(ConstraintUnsatisfiedReason::UserVersion);
        }

        let server_daemon_startup_config = daemon.daemon_startup_config.as_ref().and_then(|c| {
            match DaemonStartupConfig::deserialize(c) {
                Ok(server) => Some(server),
                Err(e) => {
                    soft_error_or_panic!(
                        "daemon_startup_config_invalid",
                        e.context("Daemon returned invalid DaemonStartupConfig")
                    );
                    None
                }
            }
        });

        let Some(server_config) = server_daemon_startup_config else {
            return Err(ConstraintUnsatisfiedReason::StartupConfig);
        };

        let changes = self
            .daemon_startup_config
            .buck_settings_changes_from(&server_config);
        if !changes.is_empty() {
            return Err(ConstraintUnsatisfiedReason::BuckSettings { changes });
        }

        if self.daemon_startup_config.startup_options != server_config.startup_options {
            return Err(ConstraintUnsatisfiedReason::StartupConfig);
        }

        if let Some(r) = &self.reject_daemon {
            if *r == daemon.daemon_id {
                return Err(ConstraintUnsatisfiedReason::RejectDaemonId);
            }
        }

        // At this point, if ExtraDaemonConstraints is missing, we'll reuse the daemon (as that
        // means it failed to start), if not we proceed to check further constraints.

        let extra = match &daemon.extra {
            Some(e) => e,
            None => return Ok(()),
        };

        match (self.desired_trace_io_state, extra.trace_io_enabled) {
            (DesiredTraceIoState::Enabled, false) => {
                return Err(ConstraintUnsatisfiedReason::TraceIo);
            }
            (DesiredTraceIoState::Disabled, true) => {
                return Err(ConstraintUnsatisfiedReason::TraceIo);
            }
            _ => {}
        }

        if let Some(r) = &self.reject_materializer_state {
            if extra.materializer_state_identity.as_ref() == Some(r) {
                return Err(ConstraintUnsatisfiedReason::SqliteIdentity);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Dupe)]
pub enum DesiredTraceIoState {
    Enabled,
    Disabled,
    Existing,
}

#[derive(Debug, Clone, Copy, Dupe)]
pub enum DaemonStartupMode {
    Standard,
    CleanStale,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum BuckdConnectOptions {
    ExistingOnly,
    Options(BuckdConnectDaemonOptions),
}

#[derive(Debug, Clone)]
pub struct BuckdConnectDaemonOptions {
    pub(crate) constraints: DaemonConstraintsRequest,
    pub(crate) daemon_startup_mode: DaemonStartupMode,
    pub(crate) new_isolation_dir_advice: AgentAdvice,
    pub(crate) trace_id: TraceId,
    /// Start the daemon by asking the installed Buck wrapper to re-exec it
    /// outside of the AI sandbox.
    ///
    /// The normal Unix daemon startup path runs the daemon executable directly:
    ///
    /// ```text
    /// <daemon-exe> --isolation-dir <dir> daemon ...
    /// ```
    ///
    /// When this is set and the current process certificate has an `agent.id`
    /// identity attribute, Buck preserves the normal daemon argv, but prefixes
    /// it with the wrapper command and the canonical daemon executable:
    ///
    /// ```text
    /// /usr/local/bin/buck unsandbox-daemon <daemon-exe> --isolation-dir <dir> daemon ...
    /// ```
    ///
    /// This _only_ works if the wrapper is installed at `/usr/local/bin/buck`,
    /// as the BPFJailer policy only allows executables with that path the
    /// ability to exit the jail. Wrappers that support the `unsandbox-daemon`
    /// are installed setuid-root, and must be from a release build on or after
    /// 20260630.
    ///
    /// The wrapper owns the privileged/sandbox-specific part of the flow: it
    /// validates that the requested executable is the Buck daemon associated
    /// with the caller, escapes the AI sandbox where supported, and then execs
    /// the daemon executable with the original daemon argv. From this client's
    /// point of view, the spawned process still follows the ordinary Unix Buck
    /// daemon startup contract: it forks, the launcher exits, and this code
    /// waits for that launcher process before connecting to the daemon socket,
    /// it just takes longer.
    ///
    /// This must remain a daemon-start-only path. The wrapper command is
    /// intentionally constructed only around Buck's normal `daemon` argv, so
    /// enabling this option should not create a general mechanism for
    /// unsandboxing arbitrary Buck commands or user-provided executables.
    #[cfg(all(fbcode_build, target_os = "linux"))]
    pub(crate) allow_daemon_start_unsandboxed_via_wrapper: bool,
}

async fn get_channel(
    endpoint: ConnectionType,
    change_to_parent_dir: bool,
) -> buck2_error::Result<Channel> {
    match endpoint {
        ConnectionType::Uds { unix_socket } => {
            Ok(get_channel_uds(&unix_socket, change_to_parent_dir).await?)
        }
        ConnectionType::Tcp { port } => Ok(get_channel_tcp(Ipv4Addr::LOCALHOST, port).await?),
    }
}

#[derive(Clone)]
pub struct BuckAddAuthTokenInterceptor {
    auth_token: AsciiMetadataValue,
}

impl Interceptor for BuckAddAuthTokenInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request
            .metadata_mut()
            .append(BUCK_AUTH_TOKEN_HEADER, self.auth_token.clone());
        Ok(request)
    }
}

pub async fn new_daemon_api_client(
    endpoint: ConnectionType,
    auth_token: String,
) -> buck2_error::Result<DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>>
{
    let channel = get_channel(endpoint, true).await?;
    Ok(DaemonApiClient::with_interceptor(
        channel,
        BuckAddAuthTokenInterceptor {
            auth_token: AsciiMetadataValue::try_from(auth_token)
                .map_err(|e| from_any_with_tag(e, ErrorTag::InvalidAuthToken))?,
        },
    )
    .max_encoding_message_size(usize::MAX)
    .max_decoding_message_size(usize::MAX))
}

// Client-side startup timeouts:
//   BUCKD_STARTUP_TIMEOUT       10 s  connect to an existing daemon (also `buck2 kill`, `--no-buckd` lock)
//   BUCKD_LAUNCH_TIMEOUT        30 s  wait for the daemon launcher to complete; falls back to BUCKD_STARTUP_TIMEOUT when that is larger
//   BUCKD_STARTUP_INIT_TIMEOUT  90 s  the whole connect-or-start sequence; bounds the other two

/// The connect timeout (`BUCKD_STARTUP_TIMEOUT`; env var name is historical).
pub fn buckd_connect_timeout() -> buck2_error::Result<Duration> {
    Ok(Duration::from_secs(
        buck2_env!("BUCKD_STARTUP_TIMEOUT", type=u64)?.unwrap_or(10),
    ))
}

/// The launcher wait (`BUCKD_LAUNCH_TIMEOUT`), clamped to the remaining startup deadline.
pub fn buckd_launch_timeout() -> buck2_error::Result<Duration> {
    Ok(launch_timeout_from(
        buck2_env!("BUCKD_LAUNCH_TIMEOUT", type=u64)?,
        buckd_connect_timeout()?,
    ))
}

/// The overall startup deadline (`BUCKD_STARTUP_INIT_TIMEOUT`).
pub fn buckd_startup_init_timeout() -> buck2_error::Result<Duration> {
    Ok(Duration::from_secs(
        buck2_env!("BUCKD_STARTUP_INIT_TIMEOUT", type=u64)?.unwrap_or(90),
    ))
}

const DEFAULT_LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// An explicit `BUCKD_LAUNCH_TIMEOUT` is taken literally.
fn launch_timeout_from(launch_timeout_secs: Option<u64>, connect_timeout: Duration) -> Duration {
    match launch_timeout_secs {
        Some(secs) => Duration::from_secs(secs),
        None => connect_timeout.max(DEFAULT_LAUNCH_TIMEOUT),
    }
}

fn launcher_wait(configured: Duration, remaining: Duration) -> Duration {
    configured.min(remaining)
}

struct ExecutableAndArgs<'a> {
    executable: OsString,
    args: Vec<Cow<'a, str>>,
}

#[cfg(not(all(fbcode_build, target_os = "linux")))]
async fn get_unix_daemon_and_args<'a>(
    _options: &BuckdConnectDaemonOptions,
    args: Vec<&'a str>,
) -> buck2_error::Result<ExecutableAndArgs<'a>> {
    Ok(ExecutableAndArgs {
        executable: get_daemon_exe()?.into_os_string(),
        args: args.into_iter().map(Cow::Borrowed).collect(),
    })
}

/// Responsible for starting the daemon when no daemon is running.
/// This struct holds a lock such that only one daemon is ever started per daemon directory.
struct BuckdLifecycle<'a> {
    paths: &'a InvocationPaths,
    lock: BuckdLifecycleLock,
}

impl<'a> BuckdLifecycle<'a> {
    async fn lock_with_timeout(
        paths: &'a InvocationPaths,
        deadline: StartupDeadline,
    ) -> buck2_error::Result<BuckdLifecycle<'a>> {
        Ok(BuckdLifecycle::<'a> {
            paths,
            lock: BuckdLifecycleLock::lock_with_timeout(paths.daemon_dir()?, deadline).await?,
        })
    }

    async fn clean_daemon_dir(&self) -> buck2_error::Result<()> {
        if buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)?
            && !shared_daemon_is_inactive(self.lock.daemon_dir()).await?
        {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Cannot verify shared daemon inactivity; no metadata was changed. Restore consistent buckd.info and buckd.pid before retrying."
            ));
        }
        self.lock
            .clean_daemon_dir(true)
            .buck_error_context("Cleaning daemon dir")
            .tag(ErrorTag::DaemonDirCleanupFailed)
    }

    async fn start_server(
        &self,
        options: &BuckdConnectDaemonOptions,
        deadline: &StartupDeadline,
    ) -> buck2_error::Result<()> {
        let constraints = &options.constraints;
        let mut args = vec!["--isolation-dir", self.paths.isolation.as_str(), "daemon"];

        let daemon_id = DaemonId::new();
        let daemon_id_s = daemon_id.to_string();

        args.push("--daemon-id");
        args.push(&daemon_id_s);

        if matches!(options.daemon_startup_mode, DaemonStartupMode::CleanStale) {
            args.push("--started-for-clean-stale");
        }

        if constraints.is_trace_io_requested() {
            args.push("--enable-trace-io");
        }

        if let Some(r) = &constraints.reject_materializer_state {
            args.push("--reject-materializer-state");
            args.push(r);
        }

        let mut daemon_env_vars = Vec::new();

        daemon_env_vars.push((OsStr::new("RUST_BACKTRACE"), OsString::from("1")));

        // TODO(nga): We create too many backtraces during `attrs.source()` coercion. Can be
        //   reproduced with this command:
        //   ```
        //   buck2 --isolation-dir=xx audit providers fbcode//buck2:buck2 --quiet
        //   ```
        //   Which regresses from 15s to 80s when `RUST_LIB_BACKTRACE` is set. So we disable
        //   backtraces in the daemon unless the user has explicitly asked for them. We
        //   intentionally avoid considering the `RUST_BACKTRACE` variables that buck was invoked
        //   with, because a lot of Rust tooling sets those without meaning to influence this
        //   behavior.
        daemon_env_vars.push((
            OsStr::new("RUST_LIB_BACKTRACE"),
            OsString::from(buck2_env!("BUCK2_LIB_BACKTRACE")?.unwrap_or("0")),
        ));

        if env::var_os("FORCE_WANT_RESTART").is_some() {
            // Disable restarter for the actual daemon command, even if it was forced, otherwise we
            // restart the daemon when it exits.
            daemon_env_vars.push((OsStr::new("FORCE_WANT_RESTART"), OsString::from("false")));
        }

        #[cfg(fbcode_build)]
        if !constraints
            .daemon_startup_config
            .startup_options
            .http
            .proxy_env_allowlist
            .is_empty()
        {
            daemon_env_vars.extend(
                HTTP_PROXY_ENV_VARS
                    .into_iter()
                    .filter_map(|name| env::var_os(name).map(|value| (OsStr::new(name), value))),
            );
        }

        if cfg!(unix) {
            // On Unix we spawn a process which forks and exits, and here we wait for that spawned
            // process to terminate. That process is usually the Buck daemon executable, but may be
            // the installed Buck wrapper, which runs the daemon on our behalf after unsandboxing it.
            let ExecutableAndArgs { executable, args } =
                get_unix_daemon_and_args(options, args).await?;

            self.start_server_unix(
                executable,
                args,
                &daemon_env_vars,
                &constraints.daemon_startup_config,
                &daemon_id,
                deadline,
            )
            .await
        } else {
            self.start_server_windows(args, &daemon_env_vars, &constraints.daemon_startup_config)
        }
    }

    fn start_server_windows(
        &self,
        mut args: Vec<&str>,
        daemon_env_vars: &[(&OsStr, OsString)],
        daemon_startup_config: &DaemonStartupConfig,
    ) -> buck2_error::Result<()> {
        let daemon_startup_config = daemon_startup_config.serialize()?;
        args.extend(["--dont-daemonize"]);
        spawn_background_process_on_windows(
            self.paths.project_root().root(),
            &get_daemon_exe()?,
            args.into_iter()
                .chain(std::iter::once(daemon_startup_config.as_str())),
            &daemon_env_vars
                .iter()
                .map(|(key, value)| (*key, value.as_os_str()))
                .collect::<Vec<_>>(),
        )
    }

    async fn start_server_unix(
        &self,
        executable: OsString,
        args: Vec<Cow<'_, str>>,
        daemon_env_vars: &[(&OsStr, OsString)],
        daemon_startup_config: &DaemonStartupConfig,
        daemon_id: &DaemonId,
        deadline: &StartupDeadline,
    ) -> buck2_error::Result<()> {
        let project_dir = self.paths.project_root();
        // The launcher gets its own budget, but never more than the overall startup deadline has
        // left; when the deadline is the smaller of the two it is the effective wait.
        let configured_timeout = buckd_launch_timeout()?;
        let timeout_secs = launcher_wait(
            configured_timeout,
            deadline.rem_duration("launching buck2 daemon")?,
        );

        // Create a unique name that we know won't overlap with other buck2 daemons and has enough
        // information to understand at least a little bit about which daemon it is
        let repo_name = project_dir
            .root()
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let unit_name = format!(
            "buck2-daemon.{}.{}.{}",
            repo_name,
            self.paths.isolation.as_str(),
            daemon_id,
        );

        let (cmd, resource_control_args) = create_daemon_spawn_command(
            &daemon_startup_config.startup_options.resource_control,
            executable,
            unit_name,
            project_dir.root(),
        )
        .await?;
        let mut cmd: tokio::process::Command = cmd.into();

        cmd.current_dir(project_dir.root())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd.args(args.iter().map(Cow::as_ref));

        cmd.arg(daemon_startup_config.serialize()?);

        cmd.args(&resource_control_args);

        if buck2_env!("BUCK_DAEMON_LOG_TO_FILE", type=u8)? == Some(1) {
            cmd.env("BUCK_LOG_TO_FILE_PATH", self.paths.log_dir().as_os_str());
        }

        for (key, val) in daemon_env_vars {
            cmd.env(key, val);
        }

        // For Unix, set a daemon process title
        #[cfg(unix)]
        {
            use std::ffi::OsString;

            let mut title = OsString::new();
            title.push("buck2d");
            if let Some(dir) = project_dir.root().file_name() {
                title.push("[");
                title.push(dir);
                title.push("]");
            }
            cmd.arg0(title);
        }

        let launch_start = Instant::now();
        let mut child = cmd.spawn()?;
        let mut stdout_taken = child
            .stdout
            .take()
            .internal_error("Child should have its stdout piped")?;
        let mut stderr_taken = child
            .stderr
            .take()
            .internal_error("Child should have its stderr piped")?;

        let status_fut = async {
            let result = timeout(timeout_secs, child.wait()).await;
            match result {
                Err(_elapsed) => {
                    // The launcher has not completed within the timeout: kill it and collect
                    // the status.
                    child.kill().await.buck_error_context(
                        "Error killing process after buck2 daemon launch timing out",
                    )?;
                    // This should return immediately as kill() waits for the process to end. We
                    // wait here again to fetch the ExitStatus.
                    let status = child
                        .wait()
                        .await
                        .buck_error_context("Daemon startup timed out")?;
                    buck2_error::Ok(LauncherExit::KilledOnTimeout {
                        status,
                        elapsed: Instant::now() - launch_start,
                    })
                }
                Ok(result) => Ok(LauncherExit::Exited(result?)),
            }
        };
        let stdout_fut = async {
            let mut buf = Vec::new();
            stdout_taken
                .read_to_end(&mut buf)
                .await
                .buck_error_context("Error reading stdout of child")?;
            Ok(buf)
        };
        let stderr_fut = async {
            let mut buf = Vec::new();
            stderr_taken
                .read_to_end(&mut buf)
                .await
                .buck_error_context("Error reading stderr of child")?;
            Ok(buf)
        };

        // `buck2 daemon` will either:
        // * fork and kill parent (daemonize) on Unix
        // * or spawn another process and exit on Windows
        // so we wait for termination of the child process.
        let joined = try_join3(status_fut, stdout_fut, stderr_fut).await;
        match joined {
            Err(error) => Err(BuckdConnectError::BuckDaemonLaunchFailed { error }.into()),
            // Our own kill: report it as a launch timeout, not as a daemon that was killed by
            // somebody else. `DAEMON_STARTUP_FAILED:SIGKILL` is left for launchers that die from
            // a signal on their own, within the timeout.
            Ok((LauncherExit::KilledOnTimeout { status, elapsed }, stdout, stderr))
                if maybe_killed_by_sigkill(status) =>
            {
                Err(BuckdConnectError::BuckDaemonLaunchTimeout {
                    timeout: timeout_secs,
                    configured: configured_timeout,
                    elapsed,
                    stdout: String::from_utf8_lossy(&stdout).to_string(),
                    stderr: String::from_utf8_lossy(&stderr).to_string(),
                }
                .into())
            }
            Ok((
                LauncherExit::KilledOnTimeout { status, .. } | LauncherExit::Exited(status),
                stdout,
                stderr,
            )) => {
                if !status.success() {
                    let exit_status_error = buck2_error::Error::from(status);
                    Err(BuckdConnectError::BuckDaemonStartupFailed {
                        stdout: String::from_utf8_lossy(&stdout).to_string(),
                        stderr: String::from_utf8_lossy(&stderr).to_string(),
                        exit_status_error,
                    }
                    .into())
                } else {
                    Ok(())
                }
            }
        }
    }
}

/// How the daemon launcher process (`buck2 daemon`, possibly behind `systemd-run` and the
/// unsandboxing wrapper) ended, as observed by `start_server_unix`.
enum LauncherExit {
    /// The launcher exited on its own within the launch timeout.
    Exited(std::process::ExitStatus),
    /// The launcher did not complete within the launch timeout and the client killed it.
    KilledOnTimeout {
        status: std::process::ExitStatus,
        /// Time from spawning the launcher to the kill.
        elapsed: Duration,
    },
}

/// Whether `status` is the status the client's own `Child::kill` (SIGKILL) produces. Anything
/// else after a timeout means the launcher raced our kill and ended on its own.
#[cfg(unix)]
fn maybe_killed_by_sigkill(status: std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    status.signal() == Some(libc::SIGKILL)
}

/// Where the exit status cannot distinguish a SIGKILL from anything else, assume that a launcher
/// we observed as `KilledOnTimeout` died from our own kill, so the launch-timeout classification
/// is reachable on every platform that ever uses this path.
#[cfg(not(unix))]
fn maybe_killed_by_sigkill(_status: std::process::ExitStatus) -> bool {
    true
}

/// The "(waited Ns)" note for `BuckDaemonLaunchTimeout`: only when the kill took effect a whole
/// second or more after the budget ran out, so the message does not repeat the budget as a
/// fake-precise measurement.
fn launch_wait_note(timeout: Duration, elapsed: Duration) -> String {
    if elapsed.as_secs() > timeout.as_secs() {
        format!(" (waited {}s)", elapsed.as_secs())
    } else {
        String::new()
    }
}

fn launch_timeout_note(timeout: Duration, configured: Duration) -> String {
    if timeout < configured {
        format!(
            " (launch timeout {}s, clamped to the remaining startup deadline)",
            configured.as_secs()
        )
    } else {
        String::new()
    }
}

/// Represents an established connection to the daemon. We then upgrade it into a
/// BootstrapBuckdClient by querying constraints. This is a separate step so that we retry
/// establishing the channel but not querying constraints.
pub struct BuckdChannel {
    info: DaemonProcessInfo,
    daemon_dir: DaemonDir,
    client: DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>,
}

impl BuckdChannel {
    /// Upgrade this BuckdChannel to a BootstrapBuckdClient.
    pub async fn upgrade(self) -> buck2_error::Result<BootstrapBuckdClient> {
        let Self {
            info,
            daemon_dir,
            mut client,
        } = self;

        let daemon_status = get_daemon_status(&mut client)
            .await
            .buck_error_context("Error obtaining daemon status")?;

        Ok(BootstrapBuckdClient {
            info,
            daemon_dir,
            client,
            constraints: daemon_status.constraints,
            daemon_start_instant: daemon_status.start_instant,
        })
    }
}

/// Client used for connection setup. Can be used to create BuckdClientConnector instances later.
#[derive(Clone)]
pub struct BootstrapBuckdClient {
    info: DaemonProcessInfo,
    daemon_dir: DaemonDir,
    client: DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>,
    /// The constraints for the daemon we're connected to.
    constraints: buck2_cli_proto::DaemonConstraints,
    daemon_start_instant: Option<Instant>,
}

impl BootstrapBuckdClient {
    /// Boxed and not inlinable: this is the entry point to the daemon connection and start-up
    /// logic, and an `async fn` here would be instantiated afresh in every command crate. See
    /// `buck2_event_log::read` for the mechanism.
    #[inline(never)]
    pub fn connect<'a>(
        paths: &'a InvocationPaths,
        options: BuckdConnectOptions,
        events_ctx: &'a mut EventsCtx,
    ) -> LocalBoxFuture<'a, buck2_error::Result<Self>> {
        Box::pin(async move {
            let daemon_dir = paths.daemon_dir()?;

            fs_util::create_dir_all(&daemon_dir.path)
                .with_buck_error_context(|| format!("Error creating daemon dir: {daemon_dir}"))?;

            let res = match &options {
                BuckdConnectOptions::ExistingOnly => {
                    establish_connection_existing(&daemon_dir).await
                }
                BuckdConnectOptions::Options(options) => {
                    establish_connection(paths, options, events_ctx).await
                }
            };

            if let Err(e) = res {
                // Advice is an intentional rejection, not a failed daemon startup.
                if e.has_tag(ErrorTag::AgentAdviceBlocked) {
                    return Err(e);
                }
                Err(daemon_connect_error(e, paths)
                    .await
                    .tag([ErrorTag::DaemonConnect]))
            } else {
                res
            }
        })
    }

    pub fn to_connector(self) -> BuckdClientConnector {
        let daemon_pid = self.info.pid;
        let cgroup_path_of_buck2_daemon = {
            #[cfg(target_os = "linux")]
            {
                buck2_resource_control::buck_cgroup_tree::read_cgroup_path_of_buck2_daemon(
                    daemon_pid,
                )
                .ok()
                .flatten()
            }
            #[cfg(not(target_os = "linux"))]
            {
                None
            }
        };
        BuckdClientConnector {
            client: BuckdClient {
                daemon_dir: self.daemon_dir,
                client: self.client,
                constraints: self.constraints,
            },
            daemon_pid,
            cgroup_path_of_buck2_daemon,
            daemon_start_instant: self.daemon_start_instant,
        }
    }

    pub(crate) async fn kill(&mut self, reason: &str) -> buck2_error::Result<Pid> {
        kill::kill(&mut self.client, &self.info, reason).await?;
        Pid::from_i64(self.info.pid)
    }

    async fn kill_for_constraints_mismatch(&mut self) -> buck2_error::Result<Pid> {
        self.kill("client expected different buckd constraints")
            .await
    }

    pub fn pid(&self) -> i64 {
        self.info.pid
    }
}

/// Attempt to connect to a daemon that can satisfy specified constraints.
/// If the daemon does not match constraints (different version or does not enable I/O tracing),
/// it will kill it and restart it with the correct constraints.
/// This behavior can be overridden by passing `BuckdConnectOptions::ExistingOnly`.
/// In that case, then any existing buck daemon (regardless of constraint) is accepted.
pub async fn connect_buckd(
    options: BuckdConnectOptions,
    events_ctx: &mut EventsCtx,
    paths: &InvocationPaths,
) -> buck2_error::Result<BuckdClientConnector> {
    match BootstrapBuckdClient::connect(paths, options, events_ctx).await {
        Ok(client) => Ok(client.to_connector()),
        Err(e) => {
            if !e.has_tag(ErrorTag::AgentAdviceBlocked) {
                events_ctx.handle_daemon_connection_failure();
            }
            Err(e)
        }
    }
}

pub async fn establish_connection_existing(
    daemon_dir: &DaemonDir,
) -> buck2_error::Result<BootstrapBuckdClient> {
    let deadline = StartupDeadline::duration_from_now(buckd_connect_timeout()?)?;
    deadline
        .run(
            "establishing connection to existing Buck daemon",
            async move {
                BuckdProcessInfo::load(daemon_dir)?
                    .create_channel()
                    .await?
                    .upgrade()
                    .await
            },
        )
        .await
}

/// Checks for an empty daemon directory or consistent metadata naming an exited process.
/// Hold lifecycle coordination when using the result to change daemon state.
pub async fn shared_daemon_is_inactive(daemon_dir: &DaemonDir) -> buck2_error::Result<bool> {
    let daemon_dir = daemon_dir.clone();
    tokio::task::spawn_blocking(move || -> buck2_error::Result<bool> {
        let Some(info) = BuckdProcessInfo::load_if_exists(&daemon_dir)? else {
            return Ok(![
                daemon_dir.buckd_pid(),
                daemon_dir.buckd_stdout(),
                daemon_dir.buckd_stderr(),
                daemon_dir.buckd_error_log(),
            ]
            .iter()
            .any(|path| path.exists()));
        };
        let pid = info.pid()?;
        let recorded_pid: u32 = fs_util::read_to_string(daemon_dir.buckd_pid())
            .categorize_internal()?
            .trim()
            .parse()?;
        Ok(recorded_pid == pid.to_u32() && !process_exists(pid)?)
    })
    .await?
    .buck_error_context(
        "Cannot verify shared daemon inactivity; restore consistent buckd.info and buckd.pid before retrying",
    )
}

async fn establish_connection(
    paths: &InvocationPaths,
    options: &BuckdConnectDaemonOptions,
    events_ctx: &mut EventsCtx,
) -> buck2_error::Result<BootstrapBuckdClient> {
    // There are many places where `establish_connection_inner` may hang.
    // If it does, better print something to the user instead of hanging quietly forever.
    let timeout = buckd_startup_init_timeout()?;
    let deadline = StartupDeadline::duration_from_now(timeout)?;
    deadline
        .down(
            "establishing connection to Buck daemon or start a daemon",
            |timeout| establish_connection_inner(paths, options, timeout, events_ctx),
        )
        .await
}

fn explain_failed_to_connect_reason(reason: buck2_data::DaemonWasStartedReason) -> &'static str {
    match reason {
        DaemonWasStartedReason::UnknownReason => "Unknown reason",
        DaemonWasStartedReason::ConstraintMismatchVersion => "Version mismatch",
        DaemonWasStartedReason::ConstraintMismatchUserVersion => "User version mismatch",
        DaemonWasStartedReason::ConstraintMismatchStartupConfig => "Startup config mismatch",
        DaemonWasStartedReason::ConstraintRejectDaemonId => "Reject daemon id",
        DaemonWasStartedReason::ConstraintMismatchTraceIo => "Trace IO mismatch",
        DaemonWasStartedReason::ConstraintMismatchSqliteIdentity => "Sqlite identity mismatch",
        DaemonWasStartedReason::CouldNotConnectToDaemon => {
            // TODO(nga): get rid of this variant.
            "Could not connect to daemon"
        }
        DaemonWasStartedReason::TimedOutConnectingToDaemon => "Timed out connecting to daemon",
        DaemonWasStartedReason::TimeoutCalculationError => "Timeout calculation error",
        DaemonWasStartedReason::NoBuckdInfo => "No buckd.info",
        DaemonWasStartedReason::CouldNotLoadBuckdInfo => "Could not load buckd.info",
        DaemonWasStartedReason::NoDaemonProcess => "buck2 daemon is not running",
    }
}

#[allow(clippy::collapsible_match)]
async fn establish_connection_inner(
    paths: &InvocationPaths,
    options: &BuckdConnectDaemonOptions,
    deadline: StartupDeadline,
    events_ctx: &mut EventsCtx,
) -> buck2_error::Result<BootstrapBuckdClient> {
    let constraints = &options.constraints;
    let daemon_dir = paths.daemon_dir()?;

    let res = deadline
        .half()?
        .run("connecting to existing buck daemon", {
            try_connect_existing_before_acquiring_lifecycle_lock(&daemon_dir, constraints).map(Ok)
        })
        .await;
    if let Ok(connect_before_restart) = res {
        if let ConnectBeforeRestart::Accepted(client) = connect_before_restart {
            return Ok(client);
        };
    }

    // At this point, we've either failed to connect to buckd or buckd had the wrong constraints.
    // Get the lifecycle lock to ensure we don't have races with other processes as we check and change things.
    let lifecycle_lock = deadline
        .down("acquire lifecycle lock", |deadline| {
            BuckdLifecycle::lock_with_timeout(paths, deadline)
        })
        .await?;

    // Even if we didn't connect before, it's possible that we just raced with another invocation
    // starting the server, so we try to connect again while holding the lock.
    let daemon_was_started_reason = {
        match BuckdProcessInfo::load_if_exists(&daemon_dir) {
            Ok(Some(buckd_info)) => {
                match try_connect_existing(&buckd_info, &deadline, &lifecycle_lock).await {
                    Ok(channel) => {
                        let mut client = channel.upgrade().await?;

                        let reason = match constraints.satisfied(&client.constraints) {
                            Ok(()) => return Ok(client),
                            Err(reason) => reason,
                        };

                        if let Some(group) = paths.shared_daemon_group()? {
                            return Err(BuckdConnectError::GroupConstraintMismatch {
                                group: group.to_string(),
                                reason,
                            }
                            .into());
                        }

                        if is_nested_invocation(
                            get_possibly_nested_invocation_daemon_uuid().as_ref(),
                            &client.constraints,
                        ) {
                            match reason {
                                ConstraintUnsatisfiedReason::TraceIo
                                | ConstraintUnsatisfiedReason::StartupConfig
                                | ConstraintUnsatisfiedReason::BuckSettings { .. } => {
                                    return Err(BuckdConnectError::NestedConstraintMismatch {
                                        reason,
                                    }
                                    .into());
                                }
                                _ => (),
                            }
                        }

                        if buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
                            return Err(buck2_error!(
                                ErrorTag::Input,
                                "Shared daemon constraint mismatch: {reason}. Stop its tenants with `buck2 killall --repo` before changing daemon startup settings or binaries."
                            ));
                        }

                        let message = match reason {
                            ConstraintUnsatisfiedReason::BuckSettings { .. } => {
                                format!("{reason}; restarting daemon...")
                            }
                            _ => format!(
                                "buck2 daemon constraint mismatch: {reason}; killing daemon..."
                            ),
                        };
                        events_ctx.eprintln(&message).await?;

                        deadline
                            .run(
                                "sending kill command to the Buck daemon",
                                client.kill_for_constraints_mismatch(),
                            )
                            .await?;

                        events_ctx.eprintln("Starting new buck2 daemon...").await?;

                        reason.to_daemon_was_started_reason()
                    }
                    Err(reason) => {
                        if buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)?
                            && process_exists(buckd_info.pid()?)?
                        {
                            return Err(buck2_error!(
                                ErrorTag::Input,
                                "Shared daemon is alive but could not be contacted: {}. No daemon was killed.",
                                explain_failed_to_connect_reason(reason)
                            ));
                        }
                        //
                        // This kill can fail with EPERM when the recorded pid belongs to another
                        // uid, or was recorded inside a pid namespace and resolves to an unrelated
                        // host process. A privileged helper (e.g. a verb on the setuid Buck
                        // wrapper) could kill such daemons from the host side, but exposing "kill
                        // a process" through a setuid binary is severe-incident territory: it
                        // would have to verify on the host that the target really is a Buck2
                        // daemon for this project root and isolation dir (unit name, /proc cwd,
                        // cmdline) before signalling, and any gap in that check is a
                        // cross-user kill primitive. Until that is designed carefully, we accept
                        // that these daemons cannot be removed from here.
                        if !buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
                            events_ctx
                                .eprintln(&format!(
                                    "Could not connect to buck2 daemon ({}), killing daemon..",
                                    explain_failed_to_connect_reason(reason)
                                ))
                                .await?;
                            if let Err(error) = hard_kill_until(&buckd_info.info, &deadline).await {
                                events_ctx
                                    .eprintln(&format!(
                                        "{}",
                                        BuckdConnectError::DaemonKillFailed { error }
                                    ))
                                    .await?;
                            }
                        }

                        reason
                    }
                }
            }
            Ok(None) => {
                // Check under the lifecycle lock after retrying the connection: another
                // invocation may have initialized this isolation directory while we waited.
                events_ctx
                    .handle_agent_advice(options.trace_id.dupe(), options.new_isolation_dir_advice)
                    .await?;
                events_ctx.eprintln("Starting new buck2 daemon...").await?;

                buck2_data::DaemonWasStartedReason::NoBuckdInfo
            }
            Err(e) => {
                events_ctx
                    .handle_agent_advice(options.trace_id.dupe(), options.new_isolation_dir_advice)
                    .await?;
                events_ctx
                    .eprintln(&format!(
                        "Could not load buckd.info: {e}, starting new buck2 daemon..."
                    ))
                    .await?;

                buck2_data::DaemonWasStartedReason::CouldNotLoadBuckdInfo
            }
        }
    };

    deadline
        .down(
            &format!(
                "starting new buck2 daemon for reason: {}",
                explain_failed_to_connect_reason(daemon_was_started_reason)
            ),
            |deadline| {
                start_new_buckd_and_connect(
                    deadline,
                    &lifecycle_lock,
                    paths,
                    options,
                    events_ctx,
                    daemon_was_started_reason,
                )
            },
        )
        .await
}

async fn start_new_buckd_and_connect(
    deadline: StartupDeadline,
    lifecycle_lock: &BuckdLifecycle<'_>,
    paths: &InvocationPaths,
    options: &BuckdConnectDaemonOptions,
    events_ctx: &mut EventsCtx,
    daemon_was_started_reason: buck2_data::DaemonWasStartedReason,
) -> buck2_error::Result<BootstrapBuckdClient> {
    let constraints = &options.constraints;

    // Daemon dir may be corrupted. Safer to delete it.
    lifecycle_lock.clean_daemon_dir().await?;

    // A previous daemon of this isolation directory may still be releasing its outputs, and the
    // new daemon cannot start until it has. A running `buck2 clean` holds the lifecycle lock, so
    // it has finished by now.
    if !buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false)? {
        let wait =
            deadline.rem_duration("waiting for the previous daemon to release its outputs")?;
        drop(TenantDiskLock::acquire(&paths.tenant_paths(), wait).await?);
    }

    // Now there's definitely no server that can be connected to
    lifecycle_lock
        .start_server(options, &deadline)
        .await
        .buck_error_context("Error starting buck2 daemon")?;
    // It might take a little bit for the daemon server to start up. We could wait for the buckd.info
    // file to appear, but it's just as easy to just retry the connection itself.

    let channel = deadline
        .retrying(
            "connect to buckd after server start",
            Duration::from_millis(5),
            Duration::from_millis(100),
            || async { BuckdProcessInfo::load_and_create_channel(&paths.daemon_dir()?).await },
        )
        .await?;

    let client = channel.upgrade().await?;

    if let Err(reason) = constraints.satisfied(&client.constraints) {
        return Err(BuckdConnectError::BuckDaemonConstraintWrongAfterStart {
            reason,
            expected: (*constraints).clone(),
            actual: client.constraints,
        }
        .into());
    }

    events_ctx.handle_daemon_started(daemon_was_started_reason);

    events_ctx
        .eprintln("Connected to new buck2 daemon.")
        .await?;

    Ok(client)
}

#[allow(clippy::large_enum_variant)]
enum ConnectBeforeRestart {
    Accepted(BootstrapBuckdClient),
    Rejected,
}

/// Connect to buckd before attempt to restart the server.
///
/// # Returns
///
/// * `Ok(Some(client))` if we connected to an existing buckd
/// * `Ok(None)` if we failed to connect and should restart buckd
/// * `Err` if we failed to connect and should abandon startup
async fn try_connect_existing_before_acquiring_lifecycle_lock(
    daemon_dir: &DaemonDir,
    constraints: &DaemonConstraintsRequest,
) -> ConnectBeforeRestart {
    match BuckdProcessInfo::load_and_create_channel(daemon_dir).await {
        Ok(channel) => {
            let Ok(client) = channel.upgrade().await else {
                return ConnectBeforeRestart::Rejected;
            };
            if constraints.satisfied(&client.constraints).is_ok() {
                ConnectBeforeRestart::Accepted(client)
            } else {
                ConnectBeforeRestart::Rejected
            }
        }
        Err(e) => {
            tracing::debug!("Connect failed: {:#}", e);
            ConnectBeforeRestart::Rejected
        }
    }
}

async fn try_connect_existing(
    buckd_info: &BuckdProcessInfo<'_>,
    timeout: &StartupDeadline,
    _lock: &BuckdLifecycle<'_>,
) -> Result<BuckdChannel, buck2_data::DaemonWasStartedReason> {
    let timeout: buck2_error::Result<_> = try { timeout.min(buckd_connect_timeout()?)? };
    let Ok(timeout) = timeout else {
        return Err(buck2_data::DaemonWasStartedReason::TimeoutCalculationError);
    };
    let Ok(rem_duration) = timeout.rem_duration("connect existing buckd") else {
        return Err(buck2_data::DaemonWasStartedReason::TimedOutConnectingToDaemon);
    };
    match tokio::time::timeout(rem_duration, buckd_info.create_channel()).await {
        Ok(Ok(channel)) => Ok(channel),
        Ok(Err(_)) => {
            let Ok(pid) = buckd_info.pid() else {
                return Err(buck2_data::DaemonWasStartedReason::CouldNotLoadBuckdInfo);
            };
            let buckd_process_exists = process_exists(pid).unwrap_or(true);
            if !buckd_process_exists {
                // We don't delete the `buckd.info` file, and if we failed to connect,
                // the most likely reason is that the daemon process doesn't exist.
                Err(buck2_data::DaemonWasStartedReason::NoDaemonProcess)
            } else {
                Err(buck2_data::DaemonWasStartedReason::CouldNotConnectToDaemon)
            }
        }
        Err(e) => {
            let _assert_type: tokio::time::error::Elapsed = e;
            Err(buck2_data::DaemonWasStartedReason::TimedOutConnectingToDaemon)
        }
    }
}

pub struct BuckdProcessInfo<'a> {
    pub(crate) info: DaemonProcessInfo,
    daemon_dir: &'a DaemonDir,
}

impl<'a> BuckdProcessInfo<'a> {
    /// Utility method for places that want to match on the overall result of those two operations.
    async fn load_and_create_channel(
        daemon_dir: &'a DaemonDir,
    ) -> buck2_error::Result<BuckdChannel> {
        Self::load(daemon_dir)?.create_channel().await
    }

    pub fn load(daemon_dir: &'a DaemonDir) -> buck2_error::Result<Self> {
        match Self::load_if_exists(daemon_dir) {
            Ok(Some(info)) => Ok(info),
            Ok(None) => Err(BuckdConnectError::BuckdInfoMissing {
                path: daemon_dir.buckd_info(),
            }
            .into()),
            Err(e) => Err(e),
        }
    }

    pub fn load_if_exists(daemon_dir: &'a DaemonDir) -> buck2_error::Result<Option<Self>> {
        let location = daemon_dir.buckd_info();
        let file = match File::open(&location) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(e).with_buck_error_context(|| {
                    format!("Trying to open buckd info, `{}`", location.display())
                });
            }
        };
        let reader = BufReader::new(file);
        let info = serde_json::from_reader(reader)
            .map_err(|error| BuckdConnectError::BuckdInfoParseError { location, error })?;

        Ok(Some(BuckdProcessInfo { info, daemon_dir }))
    }

    pub async fn create_channel(&self) -> buck2_error::Result<BuckdChannel> {
        tracing::debug!("Creating channel to: {}", self.info.endpoint);
        let connection_type = ConnectionType::parse(&self.info.endpoint)?;

        let client = new_daemon_api_client(connection_type, self.info.auth_token.clone())
            .await
            .buck_error_context("Error connecting")?;

        Ok(BuckdChannel {
            info: self.info.clone(),
            daemon_dir: self.daemon_dir.clone(),
            client,
        })
    }

    pub async fn hard_kill(&self) -> buck2_error::Result<()> {
        kill::hard_kill(&self.info).await
    }

    /// Whether the recorded pid still belongs to the daemon that wrote this record: the process
    /// must exist and have the recorded start time. A record without a start time never matches.
    pub(crate) fn recorded_daemon_is_running(&self) -> bool {
        let Ok(pid) = Pid::from_i64(self.info.pid) else {
            return false;
        };
        self.info.pid_start_time != 0
            && buck2_wrapper_common::kill::process_start_time(Some(pid))
                == Some(self.info.pid_start_time)
    }

    pub fn pid(&self) -> buck2_error::Result<Pid> {
        Pid::from_i64(self.info.pid)
    }
}

struct DaemonStatus {
    constraints: buck2_cli_proto::DaemonConstraints,
    start_instant: Option<Instant>,
}

async fn get_daemon_status(
    client: &mut DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>,
) -> buck2_error::Result<DaemonStatus> {
    // NOTE: No tailers in bootstrap client, we capture logs if we fail to connect, but
    // otherwise we leave them alone.
    let mut events_ctx = EventsCtx::new(None, vec![Box::new(StdoutStderrForwarder)]);
    // Subtracting server uptime from the request start makes RPC latency move the estimated
    // daemon start earlier, so it cannot exclude a relevant OOM record.
    let status_request_start = Instant::now();
    let status = DaemonEventsCtx::without_tailers(&mut events_ctx)
        .unpack_oneshot({
            client.status(tonic::Request::new(buck2_cli_proto::StatusRequest {
                snapshot: false,
                include_tokio_runtime_metrics: false,
            }))
        })
        .await?;

    let status: buck2_cli_proto::StatusResponse = match status {
        CommandOutcome::Success(r) => Ok(r),
        CommandOutcome::Failure(_) => Err(buck2_error!(
            ErrorTag::DaemonStatus,
            "Unexpected failure message in status()"
        )),
    }?;

    let start_instant = status
        .uptime
        .and_then(|uptime| Duration::try_from(uptime).ok())
        .and_then(|uptime| status_request_start.checked_sub(uptime));

    Ok(DaemonStatus {
        constraints: status.daemon_constraints.unwrap_or_default(),
        start_instant,
    })
}

pub fn get_daemon_exe() -> buck2_error::Result<PathBuf> {
    let exe = env::current_exe().buck_error_context("Failed to get current exe")?;
    if buck2_core::client_only::is_client_only()? {
        let ext = if cfg!(windows) { ".exe" } else { "" };
        Ok(exe
            .parent()
            .internal_error("Expected current exe to be in a directory")?
            .join(format!("buck2-daemon{ext}")))
    } else {
        Ok(exe)
    }
}

#[derive(Debug, buck2_error::Error)]
#[allow(clippy::large_enum_variant)]
#[buck2(tag = DaemonConnect)]
enum BuckdConnectError {
    #[error("buck daemon startup failed\nstdout:\n{stdout}\nstderr:\n{stderr}")]
    #[buck2(tag = DaemonStartupFailed)]
    BuckDaemonStartupFailed {
        stdout: String,
        stderr: String,
        #[source]
        exit_status_error: buck2_error::Error,
    },
    #[error("Failed to launch Buck2 daemon: {error:#}")]
    #[buck2(tag = DaemonLaunchFailed)]
    BuckDaemonLaunchFailed {
        #[source]
        error: buck2_error::Error,
    },
    #[error(
        "buck2 daemon launcher did not complete within {}s{}{}; the host may be overloaded \
         (systemd-run / cgroup setup / Eden chdir / binary page-in); killed it\nstdout:\n{stdout}\nstderr:\n{stderr}",
        .timeout.as_secs(),
        launch_wait_note(*.timeout, *.elapsed),
        launch_timeout_note(*.timeout, *.configured)
    )]
    #[buck2(tag = DaemonLaunchTimeout)]
    BuckDaemonLaunchTimeout {
        /// The effective wait (`launcher_wait`).
        timeout: Duration,
        /// The configured launch timeout (`buckd_launch_timeout`).
        configured: Duration,
        /// Spawn-to-kill time; shown only when it overshoots `timeout` by a second or more.
        elapsed: Duration,
        stdout: String,
        stderr: String,
    },
    #[error(
        "during buck daemon startup, the started process did not match constraints ({reason}).\nexpected: {expected:?}\nactual: {actual:?}"
    )]
    #[buck2(tag = DaemonConstraintsWrongAfterStart)]
    BuckDaemonConstraintWrongAfterStart {
        reason: ConstraintUnsatisfiedReason,
        expected: DaemonConstraintsRequest,
        actual: buck2_cli_proto::DaemonConstraints,
    },
    #[error("buck2 daemon constraint mismatch during nested invocation: {reason}")]
    #[buck2(tag = DaemonNestedConstraintsMismatch)]
    NestedConstraintMismatch { reason: ConstraintUnsatisfiedReason },
    #[error(
        "buck2 daemon group `{group}` has incompatible constraints: {reason}. Choose a different BUCK2_DAEMON_GROUP or stop the existing group's tenants before changing its daemon configuration."
    )]
    #[buck2(tag = Input)]
    GroupConstraintMismatch {
        group: String,
        reason: ConstraintUnsatisfiedReason,
    },
    #[error("buckd info {path} does not exist")]
    #[buck2(tag = BuckdInfoMissing)]
    BuckdInfoMissing { path: AbsNormPathBuf },
    #[error("Error parsing daemon info in `{}`. \
                Try deleting that file and running `buck2 killall` before running your command again",
                location.display())]
    #[buck2(tag = BuckdInfoParseError)]
    BuckdInfoParseError {
        location: AbsNormPathBuf,
        #[source]
        error: serde_json::Error,
    },
    #[error("Failed to kill buckd: {error:#}")]
    #[buck2(tag = DaemonKillFailed)]
    DaemonKillFailed {
        #[source]
        error: buck2_error::Error,
    },
}

async fn daemon_connect_error(
    error: buck2_error::Error,
    paths: &InvocationPaths,
) -> buck2_error::Error {
    let error_report: Result<buck2_data::ErrorReport, RetryError<buck2_error::Error>> = retrying(
        Duration::from_millis(50),
        Duration::from_millis(100),
        Duration::from_millis(500),
        || async {
            let daemon_dir = paths.daemon_dir()?;
            let error_log = std::fs::read(daemon_dir.buckd_error_log())?;

            let error_report = buck2_data::ErrorReport::deserialize(
                &mut serde_json::Deserializer::from_slice(&error_log),
            )?;
            Ok(error_report)
        },
    )
    .await;

    let error = if let Ok(error_report) = error_report {
        // Daemon wrote an error and most likely quit.
        let daemon_error: buck2_error::Error = error_report.into();
        if daemon_error.has_tag(ErrorTag::DaemonStateInitFailed) {
            // If error is in this stage of daemon init, exclude connection error details/workaround message.
            // TODO(ctolliday) always hide connection error details/workaround message if there is a structured error from daemon.
            return daemon_error;
        }
        daemon_error
    } else {
        // Daemon crashed or panicked, or is still running but can't be connected to.
        let stderr = paths
            .daemon_dir()
            .and_then(|dir| {
                let stderr = std::fs::read(dir.buckd_stderr())?;
                Ok(String::from_utf8_lossy(&stderr).into_owned())
            })
            .unwrap_or_else(|_| "<none>".to_owned());

        let stderr = truncate(&stderr, 64000);
        let error = error
            .context(format!(
                "Error connecting to the daemon, daemon stderr follows:\n{stderr}"
            ))
            .tag([ErrorTag::DaemonConnect]);

        let disk_near_full = fs_util::disk_space_stats(paths.buck_out_path())
            .is_ok_and(|stats| is_disk_near_full(stats.free_space, stats.total_space));

        classify_server_stderr(error, &stderr, disk_near_full)
    };
    if matches!(
        buck2_env!("BUCK2_SHARE_DAEMON", type=bool, default=false),
        Ok(true)
    ) {
        return error.context(
            "Failed to connect to shared buck daemon. Restore consistent buckd.info and buckd.pid before retrying if metadata is damaged.",
        );
    }
    let delete_command = if cfg!(windows) {
        "rmdir /s /q %USERPROFILE%\\.buck\\buckd"
    } else {
        "rm -rf ~/.buck/buckd"
    };
    let daemon_process_info = BuckdProcessInfoDiagnostic::new(paths);

    let kill_command = if daemon_process_info.process_exists() {
        "running `buck2 kill` and your command afterwards.
    Alternatively, try "
    } else {
        ""
    };

    let error_message = if error.has_tag(ErrorTag::DaemonLaunchTimeout) {
        // The client killed its own launcher for being slow. There is no daemon state to clean
        // up, and deleting the daemon dir does not make the next launch any faster.
        format!(
            "Failed to connect to buck daemon.
    {daemon_process_info}

    The daemon launcher was killed because it did not complete in time; try again, and if the \
    host is busy raise the launcher wait (`BUCKD_LAUNCH_TIMEOUT`, in seconds; default 30)"
        )
    } else {
        format!(
            "Failed to connect to buck daemon.
    {daemon_process_info}

    Try {kill_command}running `{delete_command}` and your command afterwards"
        )
    };
    error.context(error_message)
}

enum BuckdProcessInfoDiagnostic {
    DaemonDirUnavailable(buck2_error::Error),
    MissingBuckdInfo(DaemonDir),
    LoadError {
        daemon_dir: DaemonDir,
        error: buck2_error::Error,
    },
    ProcessInfo {
        daemon_dir: DaemonDir,
        info: DaemonProcessInfo,
        pid_present_on_system: Option<bool>,
    },
}

impl BuckdProcessInfoDiagnostic {
    fn new(paths: &InvocationPaths) -> Self {
        let daemon_dir = match paths.daemon_dir() {
            Ok(daemon_dir) => daemon_dir,
            Err(e) => return BuckdProcessInfoDiagnostic::DaemonDirUnavailable(e),
        };
        let daemon_dir_for_load = daemon_dir.clone();

        match BuckdProcessInfo::load_if_exists(&daemon_dir_for_load) {
            Ok(Some(process_info)) => BuckdProcessInfoDiagnostic::ProcessInfo {
                pid_present_on_system: pid_present_on_system(process_info.info.pid),
                daemon_dir,
                info: process_info.info,
            },
            Ok(None) => BuckdProcessInfoDiagnostic::MissingBuckdInfo(daemon_dir),
            Err(e) => BuckdProcessInfoDiagnostic::LoadError {
                daemon_dir,
                error: e,
            },
        }
    }

    fn process_exists(&self) -> bool {
        match self {
            BuckdProcessInfoDiagnostic::ProcessInfo {
                pid_present_on_system,
                ..
            } => *pid_present_on_system == Some(true),
            _ => false,
        }
    }

    fn pid_present_for_display(&self) -> &'static str {
        match self {
            BuckdProcessInfoDiagnostic::ProcessInfo {
                pid_present_on_system,
                ..
            } => match pid_present_on_system {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            },
            _ => "unknown",
        }
    }
}

// N.B. that if the daemon has already exited and the pid has been reused, this
// may give incorrect results.
fn pid_present_on_system(pid: i64) -> Option<bool> {
    Pid::from_i64(pid)
        .ok()
        .and_then(|pid| process_exists(pid).ok())
}

impl Display for BuckdProcessInfoDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuckdProcessInfoDiagnostic::ProcessInfo {
                daemon_dir, info, ..
            } => write!(
                f,
                "Daemon process info from {}:
    daemon dir: {}
    pid: {}
    endpoint: {}
    version: {}
    pid present on system: {}",
                daemon_dir.buckd_info(),
                daemon_dir.path,
                info.pid,
                info.endpoint,
                info.version,
                self.pid_present_for_display(),
            ),
            BuckdProcessInfoDiagnostic::MissingBuckdInfo(daemon_dir) => write!(
                f,
                "Daemon process info from {}:
    daemon dir: {}
    status: <missing>",
                daemon_dir.buckd_info(),
                daemon_dir.path,
            ),
            BuckdProcessInfoDiagnostic::LoadError { daemon_dir, error } => write!(
                f,
                "Daemon process info from {}:
    daemon dir: {}
    status: unavailable: {error:#}",
                daemon_dir.buckd_info(),
                daemon_dir.path,
            ),
            BuckdProcessInfoDiagnostic::DaemonDirUnavailable(error) => {
                write!(f, "Daemon process info unavailable: {error:#}")
            }
        }
    }
}

fn is_nested_invocation(
    buck2_daemon_uuid: Option<&String>,
    daemon: &buck2_cli_proto::DaemonConstraints,
) -> bool {
    buck2_daemon_uuid == Some(&daemon.daemon_id)
}

#[cfg(test)]
mod tests {
    use buck2_error::internal_error;
    use buck2_settings::BuckSettings;

    use super::*;

    fn constraints(trace_io_enabled: bool) -> buck2_cli_proto::DaemonConstraints {
        buck2_cli_proto::DaemonConstraints {
            version: "version".to_owned(),
            user_version: Some("test".to_owned()),
            daemon_id: "foo".to_owned(),
            extra: Some(buck2_cli_proto::ExtraDaemonConstraints {
                trace_io_enabled,
                materializer_state_identity: None,
            }),
            daemon_startup_config: Some(
                serde_json::to_string(&DaemonStartupConfig::testing_empty()).unwrap(),
            ),
        }
    }

    fn request(desired_trace_io_state: DesiredTraceIoState) -> DaemonConstraintsRequest {
        DaemonConstraintsRequest {
            version: "version".to_owned(),
            user_version: Some("test".to_owned()),
            desired_trace_io_state,
            nested_invocation_daemon_uuid: None,
            reject_daemon: None,
            reject_materializer_state: None,
            daemon_startup_config: DaemonStartupConfig::testing_empty(),
        }
    }

    fn request_with_settings(settings: &str) -> DaemonConstraintsRequest {
        let mut req = request(DesiredTraceIoState::Existing);
        let mut startup = serde_json::to_value(&req.daemon_startup_config).unwrap();
        let settings: BuckSettings = serde_json::from_str(settings).unwrap();
        startup["buck_settings"] = serde_json::to_value(settings.daemon).unwrap();
        req.daemon_startup_config = serde_json::from_value(startup).unwrap();
        req
    }

    /// The client's own kill of a slow launcher and a launcher killed by somebody else must land
    /// under different category keys.
    #[cfg(unix)]
    #[test]
    fn test_launcher_kill_category_keys() {
        use std::os::unix::process::ExitStatusExt;

        let killed_by_client: buck2_error::Error = BuckdConnectError::BuckDaemonLaunchTimeout {
            timeout: Duration::from_secs(10),
            configured: Duration::from_secs(30),
            elapsed: Duration::from_millis(10_020),
            stdout: String::new(),
            stderr: String::new(),
        }
        .into();
        assert!(killed_by_client.has_tag(ErrorTag::DaemonLaunchTimeout));
        assert_eq!(killed_by_client.category_key(), "DAEMON_LAUNCH_TIMEOUT");
        let message = format!("{:#}", killed_by_client);
        assert!(message.contains("did not complete within 10s"), "{message}");
        assert!(message.contains("launch timeout 30s"), "{message}");
        // 10.02 s against a 10 s budget is not worth repeating.
        assert!(!message.contains("waited"), "{message}");
        assert_eq!(
            launch_wait_note(Duration::from_secs(10), Duration::from_millis(13_400)),
            " (waited 13s)"
        );
        assert_eq!(
            launch_wait_note(Duration::from_secs(10), Duration::from_millis(10_900)),
            ""
        );

        let sigkill = std::process::ExitStatus::from_raw(libc::SIGKILL);
        assert!(maybe_killed_by_sigkill(sigkill));
        assert!(!maybe_killed_by_sigkill(
            std::process::ExitStatus::from_raw(libc::SIGTERM)
        ));
        assert!(!maybe_killed_by_sigkill(
            std::process::ExitStatus::from_raw(0)
        ));
        let killed_by_someone_else: buck2_error::Error =
            BuckdConnectError::BuckDaemonStartupFailed {
                stdout: String::new(),
                stderr: String::new(),
                exit_status_error: buck2_error::Error::from(sigkill),
            }
            .into();
        assert_eq!(
            killed_by_someone_else.category_key(),
            "DAEMON_STARTUP_FAILED:SIGKILL"
        );
    }

    #[test]
    fn test_launch_timeout_message() {
        for (timeout, configured, expected_note) in [
            (Duration::from_secs(30), Duration::from_secs(30), ""),
            (Duration::from_secs(120), Duration::from_secs(120), ""),
            (Duration::ZERO, Duration::ZERO, ""),
            (
                Duration::from_secs(12),
                Duration::from_secs(30),
                " (launch timeout 30s, clamped to the remaining startup deadline)",
            ),
            (
                Duration::from_millis(29_900),
                Duration::from_secs(30),
                " (launch timeout 30s, clamped to the remaining startup deadline)",
            ),
            (
                Duration::ZERO,
                Duration::from_secs(30),
                " (launch timeout 30s, clamped to the remaining startup deadline)",
            ),
        ] {
            let error: buck2_error::Error = BuckdConnectError::BuckDaemonLaunchTimeout {
                timeout,
                configured,
                elapsed: timeout,
                stdout: String::new(),
                stderr: String::new(),
            }
            .into();
            let message = format!("{error:#}");
            assert!(
                message.starts_with(&format!(
                    "buck2 daemon launcher did not complete within {}s{expected_note}; the host",
                    timeout.as_secs()
                )),
                "{message}"
            );
            assert_eq!(error.category_key(), "DAEMON_LAUNCH_TIMEOUT");
        }
    }

    /// The launcher wait is its own knob: 30 s by default, a raised `BUCKD_STARTUP_TIMEOUT` when
    /// that is larger (it used to bound the launcher), `BUCKD_LAUNCH_TIMEOUT` when set; and it
    /// is clamped to whatever the overall startup deadline has left.
    #[test]
    fn test_launcher_wait() {
        if std::env::var_os("BUCKD_LAUNCH_TIMEOUT").is_none()
            && std::env::var_os("BUCKD_STARTUP_TIMEOUT").is_none()
        {
            assert_eq!(buckd_launch_timeout().unwrap(), Duration::from_secs(30));
            assert_eq!(buckd_connect_timeout().unwrap(), Duration::from_secs(10));
        }
        // Unset: the default, unless `BUCKD_STARTUP_TIMEOUT` was raised above it.
        assert_eq!(
            launch_timeout_from(None, Duration::from_secs(10)),
            Duration::from_secs(30)
        );
        assert_eq!(
            launch_timeout_from(None, Duration::from_secs(0)),
            Duration::from_secs(30)
        );
        assert_eq!(
            launch_timeout_from(None, Duration::from_secs(120)),
            Duration::from_secs(120)
        );
        // Set: taken literally, even below the startup timeout.
        assert_eq!(
            launch_timeout_from(Some(0), Duration::from_secs(120)),
            Duration::ZERO
        );
        assert_eq!(
            launch_timeout_from(Some(45), Duration::from_secs(10)),
            Duration::from_secs(45)
        );
        assert_eq!(
            launcher_wait(Duration::from_secs(30), Duration::from_secs(90)),
            Duration::from_secs(30)
        );
        assert_eq!(
            launcher_wait(Duration::from_secs(30), Duration::from_secs(12)),
            Duration::from_secs(12)
        );
        assert_eq!(
            launcher_wait(Duration::from_secs(0), Duration::from_secs(90)),
            Duration::ZERO
        );
    }

    #[test]
    fn test_constraints_equal_for_same_constraints() {
        let req = request(DesiredTraceIoState::Enabled);
        let daemon = constraints(true);
        assert!(req.satisfied(&daemon).is_ok());
    }

    #[test]
    fn test_buck_settings_mismatch() {
        for settings in [
            r#"{"analysis":{"record_requested_anon_targets":false}}"#,
            r#"{"hydration":{"enable_paging":true}}"#,
            r#"{"log_download":{"log_url":"https://example.com/"}}"#,
            r#"{"package_visibility":{"default_intersection":"enforce"}}"#,
        ] {
            let req = request_with_settings(settings);
            let daemon = constraints(false);

            let reason = req.satisfied(&daemon).unwrap_err();
            assert!(matches!(
                reason,
                ConstraintUnsatisfiedReason::BuckSettings { .. }
            ));
            assert_eq!(
                reason.to_daemon_was_started_reason(),
                DaemonWasStartedReason::ConstraintMismatchStartupConfig,
            );
        }
    }

    #[test]
    fn test_buck_settings_details_survive_error_chain() {
        let req = request_with_settings(
            r#"{"hydration":{"enable_paging":true},"agent_materialization":{"enabled":true}}"#,
        );
        let daemon = constraints(false);

        let reason = req.satisfied(&daemon).unwrap_err();
        let expected = "Buck settings changed (hydration.enable_paging: null -> true)";
        assert_eq!(reason.to_string(), expected);
        let error: buck2_error::Error =
            BuckdConnectError::NestedConstraintMismatch { reason }.into();
        assert!(format!("{error:#}").contains(expected));
    }

    #[test]
    fn test_invocation_sections_reuse_daemon() {
        for settings in [
            r#"{"agent_materialization":{"enabled":true}}"#,
            r#"{"agent_materialization":{"enabled":false}}"#,
            r#"{"agent_interactive":{"enabled":true,"killall_enabled":true,"clean_enabled":true,"new_isolation_dir_enabled":true}}"#,
            r#"{"agent_interactive":{"enabled":false}}"#,
            r#"{"agent_config_change":{"enabled":true}}"#,
        ] {
            let req = request_with_settings(settings);
            let mut daemon = constraints(false);
            assert!(req.satisfied(&daemon).is_ok(), "{settings}");

            daemon.daemon_startup_config = Some(req.daemon_startup_config.serialize().unwrap());
            let req = request(DesiredTraceIoState::Existing);
            assert!(req.satisfied(&daemon).is_ok(), "{settings}");
        }
    }

    #[test]
    fn test_missing_or_invalid_startup_config() {
        let req = request(DesiredTraceIoState::Existing);
        for config in [None, Some("{}"), Some("invalid json")] {
            let mut daemon = constraints(false);
            daemon.daemon_startup_config = config.map(str::to_owned);
            assert!(matches!(
                req.satisfied(&daemon),
                Err(ConstraintUnsatisfiedReason::StartupConfig)
            ));
        }
    }

    #[test]
    fn test_constraints_equal_for_trace_io_existing() {
        let req = request(DesiredTraceIoState::Existing);
        let daemon = constraints(true);
        assert!(req.satisfied(&daemon).is_ok());
    }

    #[test]
    fn test_constraints_unequal_for_trace_io() {
        let req = request(DesiredTraceIoState::Disabled);
        let daemon = constraints(true);
        assert!(req.satisfied(&daemon).is_err());
    }

    #[test]
    fn test_trace_io_is_enabled() {
        let c = request(DesiredTraceIoState::Enabled);
        assert!(c.is_trace_io_requested());

        let c = request(DesiredTraceIoState::Disabled);
        assert!(!c.is_trace_io_requested());
    }

    #[test]
    fn test_reject_daemon() {
        let mut req = DaemonConstraintsRequest {
            version: "foo".to_owned(),
            user_version: None,
            desired_trace_io_state: DesiredTraceIoState::Existing,
            reject_daemon: None,
            nested_invocation_daemon_uuid: None,
            reject_materializer_state: None,
            daemon_startup_config: DaemonStartupConfig::testing_empty(),
        };

        let daemon = buck2_cli_proto::DaemonConstraints {
            version: "foo".to_owned(),
            user_version: None,
            daemon_id: "ddd".to_owned(),
            extra: None,
            daemon_startup_config: Some(
                serde_json::to_string(&DaemonStartupConfig::testing_empty()).unwrap(),
            ),
        };

        assert!(req.satisfied(&daemon).is_ok());
        req.reject_daemon = Some("zzz".to_owned());
        assert!(req.satisfied(&daemon).is_ok());
        req.reject_daemon = Some("ddd".to_owned());
        assert!(req.satisfied(&daemon).is_err());
    }

    #[test]
    fn test_reject_materializer_state() {
        let mut req = DaemonConstraintsRequest {
            version: "foo".to_owned(),
            user_version: None,
            desired_trace_io_state: DesiredTraceIoState::Existing,
            reject_daemon: None,
            nested_invocation_daemon_uuid: None,
            reject_materializer_state: None,
            daemon_startup_config: DaemonStartupConfig::testing_empty(),
        };

        let daemon = buck2_cli_proto::DaemonConstraints {
            version: "foo".to_owned(),
            user_version: None,
            daemon_id: "ddd".to_owned(),
            extra: Some(buck2_cli_proto::ExtraDaemonConstraints {
                trace_io_enabled: false,
                materializer_state_identity: Some("mmm".to_owned()),
            }),
            daemon_startup_config: Some(
                serde_json::to_string(&DaemonStartupConfig::testing_empty()).unwrap(),
            ),
        };

        assert!(req.satisfied(&daemon).is_ok());
        req.reject_materializer_state = Some("zzz".to_owned());
        assert!(req.satisfied(&daemon).is_ok());
        req.reject_materializer_state = Some("mmm".to_owned());
        assert!(req.satisfied(&daemon).is_err());
    }

    #[test]
    fn test_daemon_buster() {
        let mut req = DaemonConstraintsRequest {
            version: "foo".to_owned(),
            user_version: None,
            desired_trace_io_state: DesiredTraceIoState::Existing,
            reject_daemon: None,
            nested_invocation_daemon_uuid: None,
            reject_materializer_state: None,
            daemon_startup_config: DaemonStartupConfig::testing_empty(),
        };

        let daemon = buck2_cli_proto::DaemonConstraints {
            version: "foo".to_owned(),
            user_version: None,
            daemon_id: "ddd".to_owned(),
            extra: Some(buck2_cli_proto::ExtraDaemonConstraints {
                trace_io_enabled: false,
                materializer_state_identity: Some("mmm".to_owned()),
            }),
            daemon_startup_config: Some(
                serde_json::to_string(&DaemonStartupConfig::testing_empty()).unwrap(),
            ),
        };

        assert!(req.satisfied(&daemon).is_ok());
        req.daemon_startup_config.startup_options.daemon_buster = Some("1".to_owned());
        assert!(matches!(
            req.satisfied(&daemon),
            Err(ConstraintUnsatisfiedReason::StartupConfig)
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn test_format_daemon_process_info_active() {
        let pid = i64::from(std::process::id());
        let process_info = DaemonProcessInfo {
            pid,
            endpoint: "tcp:44805".to_owned(),
            version: "92ca877522e06be7580a7ea2d622a3c06712322a".to_owned(),
            auth_token: "redacted".to_owned(),
            pid_start_time: 0,
        };
        let daemon_dir = DaemonDir {
            path: AbsNormPathBuf::new("/tmp/buckd".into()).expect("daemon dir should be valid"),
        };
        let wrapper = BuckdProcessInfoDiagnostic::ProcessInfo {
            daemon_dir,
            info: process_info,
            pid_present_on_system: Some(true),
        };

        assert_eq!(
            wrapper.to_string(),
            format!(
                "Daemon process info from /tmp/buckd/buckd.info:\n    daemon dir: /tmp/buckd\n    pid: {pid}\n    endpoint: tcp:44805\n    version: 92ca877522e06be7580a7ea2d622a3c06712322a\n    pid present on system: yes"
            )
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_format_daemon_process_info_missing_pid() {
        let process_info = DaemonProcessInfo {
            pid: 999_999_999,
            endpoint: "tcp:44805".to_owned(),
            version: "92ca877522e06be7580a7ea2d622a3c06712322a".to_owned(),
            auth_token: "redacted".to_owned(),
            pid_start_time: 0,
        };
        let daemon_dir = DaemonDir {
            path: AbsNormPathBuf::new("/tmp/buckd".into()).expect("daemon dir should be valid"),
        };
        let wrapper = BuckdProcessInfoDiagnostic::ProcessInfo {
            daemon_dir,
            info: process_info,
            pid_present_on_system: Some(false),
        };

        assert_eq!(
            wrapper.to_string(),
            "Daemon process info from /tmp/buckd/buckd.info:\n    daemon dir: /tmp/buckd\n    pid: 999999999\n    endpoint: tcp:44805\n    version: 92ca877522e06be7580a7ea2d622a3c06712322a\n    pid present on system: no"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_format_daemon_process_info_missing_buckd_info() {
        let daemon_dir = DaemonDir {
            path: AbsNormPathBuf::new("/tmp/buckd".into()).expect("daemon dir should be valid"),
        };

        assert_eq!(
            BuckdProcessInfoDiagnostic::MissingBuckdInfo(daemon_dir).to_string(),
            "Daemon process info from /tmp/buckd/buckd.info:\n    daemon dir: /tmp/buckd\n    status: <missing>"
        );
    }

    #[test]
    fn test_format_daemon_process_info_daemon_dir_unavailable() {
        assert_eq!(
            BuckdProcessInfoDiagnostic::DaemonDirUnavailable(internal_error!("no daemon dir"))
                .to_string(),
            "Daemon process info unavailable: no daemon dir (internal error)"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_format_daemon_process_info_load_error() {
        let daemon_dir = DaemonDir {
            path: AbsNormPathBuf::new("/tmp/buckd".into()).expect("daemon dir should be valid"),
        };

        assert_eq!(
            BuckdProcessInfoDiagnostic::LoadError {
                daemon_dir,
                error: internal_error!("failed to load buckd.info"),
            }
            .to_string(),
            "Daemon process info from /tmp/buckd/buckd.info:\n    daemon dir: /tmp/buckd\n    status: unavailable: failed to load buckd.info (internal error)"
        );
    }

    #[test]
    fn test_constraints_nested_invocation_diff_user_version() {
        let mut req = DaemonConstraintsRequest {
            version: "foo".to_owned(),
            user_version: Some("fake_version_1".to_owned()),
            desired_trace_io_state: DesiredTraceIoState::Existing,
            nested_invocation_daemon_uuid: None,
            reject_daemon: None,
            reject_materializer_state: None,
            daemon_startup_config: DaemonStartupConfig::testing_empty(),
        };

        let daemon = buck2_cli_proto::DaemonConstraints {
            version: "foo".to_owned(),
            user_version: Some("fake_version_2".to_owned()),
            daemon_id: "ddd".to_owned(),
            extra: Some(buck2_cli_proto::ExtraDaemonConstraints {
                trace_io_enabled: false,
                materializer_state_identity: Some("mmm".to_owned()),
            }),
            daemon_startup_config: Some(
                serde_json::to_string(&DaemonStartupConfig::testing_empty()).unwrap(),
            ),
        };

        assert!(req.satisfied(&daemon).is_err());

        req.nested_invocation_daemon_uuid = Some("ddd".to_owned());
        assert!(req.satisfied(&daemon).is_ok());
    }
}
