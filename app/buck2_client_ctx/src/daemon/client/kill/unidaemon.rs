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
use buck2_common::daemon_compatibility::TENANT_PROXY_SPEC_FILE;
use buck2_common::daemon_dir::DaemonDir;
use buck2_fs::paths::file_name::FileName;

use crate::daemon::client::BuckdLifecycleLock;
use crate::daemon::client::connect::BuckdProcessInfo;
use crate::daemon::client::tenant_slot::SlotOwner;
use crate::daemon::client::tenant_slot::probe_slot_dir;

/// How long the shared daemon may take to evict the generation before its watchdog ends the whole
/// daemon. A released client gives the proxy its four-second kill grace, and the proxy keeps a
/// second of it for forcing the daemon; every caller here holds the tenant directory's lock for
/// the whole eviction, so nothing forces a deadline that short.
const EVICTION_TIMEOUT: Duration = Duration::from_secs(10);

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
