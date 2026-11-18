/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Commands that stop a tenant under daemon sharing stop whatever owns the tenant's own daemon
//! directory, as a client of any release does. For a shared tenant that is the proxy of its
//! generation, whose Kill evicts exactly that generation from the shared daemon.

use std::io::ErrorKind;
use std::time::Duration;

use buck2_cli_proto::DaemonProcessInfo;
use buck2_cli_proto::KillRequest;
use buck2_cli_proto::daemon_api_client::DaemonApiClient;
use buck2_common::daemon_compatibility::TENANT_PROXY_SPEC_FILE;
use buck2_common::daemon_dir::DaemonDir;
#[cfg(target_os = "linux")]
use buck2_error::ErrorTag;
#[cfg(target_os = "linux")]
use buck2_error::buck2_error;
use buck2_fs::paths::file_name::FileName;
use buck2_wrapper_common::kill;
use buck2_wrapper_common::pid::Pid;
use tonic::Request;
use tonic::codegen::InterceptedService;
use tonic::transport::Channel;

use crate::daemon::client::BuckdLifecycleLock;
use crate::daemon::client::connect::BuckAddAuthTokenInterceptor;
use crate::daemon::client::connect::BuckdProcessInfo;
use crate::daemon::client::tenant_slot::SlotOwner;
use crate::daemon::client::tenant_slot::probe_slot_dir;

/// How long the shared daemon may take to evict the generation before its watchdog ends the whole
/// daemon. A released client gives the proxy its four-second kill grace, and the proxy keeps a
/// second of it for forcing the daemon; every caller here holds the tenant directory's lock for
/// the whole eviction, so nothing forces a deadline that short.
const EVICTION_TIMEOUT: Duration = Duration::from_secs(10);

/// Stops a shared daemon whose process-wide state is corrupt, together with its actions: the
/// forkserver and working directory are shared by every tenant, so nothing of it may survive into
/// the replacement. `client` is the channel this client already holds to the daemon of its
/// namespace and `info` that daemon's own connection record; the daemon is addressed as the
/// client found it, not through the tenant's directory, whose proxy is one of its casualties.
pub(crate) async fn stop_unidaemon(
    client: &mut DaemonApiClient<InterceptedService<Channel, BuckAddAuthTokenInterceptor>>,
    info: &DaemonProcessInfo,
) -> buck2_error::Result<()> {
    let callers = tokio::task::spawn_blocking(super::get_callers_for_kill).await?;
    let started = std::time::Instant::now();
    let request = client.kill(Request::new(KillRequest {
        reason: "Client reported corrupted daemon state".to_owned(),
        timeout: Some(super::GRACEFUL_SHUTDOWN_TIMEOUT.try_into()?),
        callers,
    }));
    match tokio::time::timeout(super::KILL_REQUEST_TIMEOUT, request).await {
        Ok(Ok(_)) => {
            let remaining = super::GRACEFUL_SHUTDOWN_TIMEOUT
                .saturating_sub(std::time::Instant::now() - started);
            if matches!(
                tokio::time::timeout(remaining, wait_for_exit(info)).await,
                Ok(Ok(()))
            ) {
                return Ok(());
            }
        }
        Ok(Err(error)) => tracing::warn!(%error, "Unidaemon shutdown request failed"),
        Err(error) => tracing::warn!(%error, "Unidaemon shutdown request timed out"),
    }
    #[cfg(not(target_os = "linux"))]
    return super::hard_kill(info).await;
    #[cfg(target_os = "linux")]
    {
        let info = info.clone();
        tokio::task::spawn_blocking(move || -> buck2_error::Result<()> {
            if kill::kill_unidaemon_tree(
                Pid::from_i64(info.pid)?,
                info.pid_start_time,
                super::FORCE_SHUTDOWN_TIMEOUT,
            )? {
                return Ok(());
            }
            Err(buck2_error!(
                ErrorTag::DaemonWontDieFromKill,
                "Could not verify termination of rejected unidaemon {} and its actions",
                info.pid
            ))
        })
        .await??;
        Ok(())
    }
}

async fn wait_for_exit(info: &DaemonProcessInfo) -> buck2_error::Result<()> {
    let pid = Pid::from_i64(info.pid)?;
    loop {
        let start_time = info.pid_start_time;
        // Judged as every reader of a connection record judges its process: a recorded start
        // time identifies it within the tolerance for clock adjustment, and one the daemon could
        // not record leaves the pid to speak for it, within the caller's timeout.
        let alive = tokio::task::spawn_blocking(move || {
            kill::recorded_process_may_be_running(pid, start_time, None)
        })
        .await??;
        if !alive {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The command stopping a tenant.
#[derive(Clone, Copy)]
pub enum TenantShutdownCommand {
    Kill,
    Clean,
    /// A command whose constraints hash to another namespace than the tenant's current one.
    Replace,
    /// `killall --repo`.
    Repository,
    /// The tenant's daemon was rejected as corrupt and restarted.
    Rejected,
}

impl TenantShutdownCommand {
    fn reason(self) -> &'static str {
        match self {
            Self::Kill => "`buck kill` was invoked",
            Self::Clean => "`buck2 clean` was invoked",
            Self::Replace => "client expected different buckd constraints",
            Self::Repository => "Repository tenant eviction",
            Self::Rejected => "The rejected unidaemon exited",
        }
    }

    /// Whether the stop happens on the way to another command, which has its own output.
    fn quiet(self) -> bool {
        matches!(self, Self::Replace | Self::Rejected)
    }
}

/// Stops the owner of the tenant's own daemon directory. A proxy is asked to evict its generation
/// with this client's budget and waited for; anything else is killed as any client kills it. With
/// `expected_daemon`, only the proxy of that daemon, or an owner that does not answer, is stopped;
/// a directory that another daemon's proxy has taken over meanwhile is left alone. The caller
/// holds the directory's lifecycle lock through this and any file deletion that follows.
pub async fn kill_unidaemon_tenant(
    lifecycle: &BuckdLifecycleLock,
    command: TenantShutdownCommand,
    expected_daemon: Option<&str>,
) -> buck2_error::Result<()> {
    let proxy = match (
        probe_slot_dir(lifecycle.daemon_dir()).await?,
        expected_daemon,
    ) {
        (SlotOwner::Proxy(proxy), expected)
            if expected.is_none_or(|daemon_id| daemon_id == proxy.daemon_id) =>
        {
            proxy
        }
        (SlotOwner::Proxy(_) | SlotOwner::Legacy(_) | SlotOwner::Vacant, Some(_)) => {
            return Ok(());
        }
        _ => {
            return super::kill_command_impl(lifecycle, command.reason(), command.quiet()).await;
        }
    };
    let info = proxy.client.process_info().clone();
    let generation = proxy.generation;
    let mut client = proxy.client;
    if !command.quiet() {
        crate::eprintln!("evicting tenant generation {}", generation)?;
    }
    // The proxy answers Kill once the eviction is accepted, exits when it has completed, and after
    // the daemon when it was the final tenant; waiting for the process covers all three.
    client
        .kill_with_timeout(command.reason(), EVICTION_TIMEOUT + Duration::from_secs(1))
        .await?;
    // The proxy removes its connection files as it exits; one that had to be stopped leaves them.
    // The eviction is done by now, so leftovers are not worth failing the command over: the next
    // client finds a record naming a dead process and treats the directory as vacant.
    let directory = lifecycle.daemon_dir().clone();
    let removed = tokio::task::spawn_blocking(move || remove_slot_files(&directory, &info))
        .await
        .map_err(buck2_error::Error::from)
        .and_then(|removed| removed);
    if let Err(error) = removed {
        tracing::warn!(
            "Left the stopped proxy's connection files in `{}`: {error:#}",
            lifecycle.daemon_dir()
        );
    }
    if !command.quiet() {
        crate::eprintln!("Evicted tenant generation {}", generation)?;
    }
    Ok(())
}

/// Removes the connection files of `proxy`, which has exited, if the directory still names it.
fn remove_slot_files(directory: &DaemonDir, proxy: &DaemonProcessInfo) -> buck2_error::Result<()> {
    let Some(current) = BuckdProcessInfo::load_if_exists(directory)? else {
        return Ok(());
    };
    if current.info != *proxy || current.may_be_running()? {
        return Ok(());
    }
    for path in [
        directory.buckd_info(),
        directory.buckd_pid(),
        directory
            .path
            .join(FileName::unchecked_new(TENANT_PROXY_SPEC_FILE)),
    ] {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
