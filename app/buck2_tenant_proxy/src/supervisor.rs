/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The shared daemon's watch over a launched proxy: its liveness link, and what happens to the
//! generation when the link ends.

use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_common::daemon_compatibility::UnidaemonTenantReference;
use buck2_common::daemon_dir::DaemonDir;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_server::daemon::proxies::ProxyEviction;
use buck2_server::daemon::proxies::ProxySupervisor;
use buck2_util::threads::thread_spawn;

use crate::emergency::emergency_exit;
use crate::events::record_proxy_event;
use crate::launch::PROXY_STARTUP_TIMEOUT;
use crate::launch::ProxyChild;
use crate::registry::ProxyExit;
use crate::registry::ProxyKey;
use crate::registry::forget_retire;
use crate::registry::take_recovery_budget;
use crate::slot::legacy_lock;
use crate::slot::slot_files;
use crate::slot::slot_matches;

/// What the supervisor of one proxy holds besides its liveness link.
pub(crate) struct Watched {
    pub(crate) child: Arc<Mutex<ProxyChild>>,
    pub(crate) spec: TenantProxySpec,
    pub(crate) directory: DaemonDir,
    pub(crate) key: ProxyKey,
    pub(crate) pid: i64,
    pub(crate) lost: Arc<AtomicBool>,
    pub(crate) exit: Arc<ProxyExit>,
    pub(crate) supervisor: ProxySupervisor,
}

pub(crate) fn watch(socket: UnixStream, watched: Watched) -> buck2_error::Result<()> {
    thread_spawn("buck2-proxy-supervisor", move || supervise(socket, watched))?;
    Ok(())
}

fn supervise(socket: UnixStream, watched: Watched) {
    let mut reader = BufReader::new(socket);
    let mut graceful = false;
    let liveness = follow_liveness(&mut reader, &watched.spec, watched.pid, &mut graceful);
    watched.lost.store(true, Ordering::Release);
    forget_retire(&watched.key);
    watched.exit.signal();
    let killed = !graceful && proxy_was_killed(&watched.child);
    record_proxy_event(
        &watched.spec,
        watched.pid,
        "proxy-liveness-lost",
        if graceful {
            "shutdown-intent"
        } else if killed {
            "killed"
        } else if liveness.is_err() {
            "monitor-error"
        } else {
            "unexpected-eof"
        },
    );
    let _runtime = watched.supervisor.runtime.enter();
    // A proxy that `buck2 killall` or a user stopped with a signal stays stopped: its tenant
    // retires and is not registered again until a client asks for it.
    let requested_recovery = !graceful && !killed;
    let eviction =
        match (watched.supervisor.begin_eviction)(watched.spec.clone(), requested_recovery) {
            Ok(eviction) => eviction,
            Err(_) => emergency_exit(),
        };
    if let Some(eviction) = eviction {
        retire_after_loss(&watched, eviction, requested_recovery);
    }
}

/// Verifies the proxy's identity over the liveness link, then follows the link until it ends.
/// `graceful` is set when the proxy announces that it is shutting down on purpose.
fn follow_liveness(
    reader: &mut BufReader<UnixStream>,
    spec: &TenantProxySpec,
    pid: i64,
    graceful: &mut bool,
) -> buck2_error::Result<()> {
    // The proxy identifies itself as soon as it starts, within the launcher's startup budget.
    reader
        .get_mut()
        .set_read_timeout(Some(PROXY_STARTUP_TIMEOUT))?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reference: UnidaemonTenantReference = serde_json::from_str(&line)?;
    if reference != spec.reference {
        return Err(buck2_error!(
            ErrorTag::Input,
            "Proxy liveness identity mismatch"
        ));
    }
    reader.get_mut().write_all(b"A")?;
    reader.get_mut().set_read_timeout(None)?;
    record_proxy_event(spec, pid, "proxy-monitor-attached", "identity-verified");
    loop {
        let mut message = [0];
        match reader.read(&mut message) {
            Ok(0) => return Ok(()),
            Ok(_) if message[0] == b'K' => *graceful = true,
            Ok(_) => {
                return Err(buck2_error!(
                    ErrorTag::Input,
                    "Invalid proxy liveness message"
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

/// Sees the generation's eviction through once its proxy is gone, then either clears the slot
/// or replaces the proxy with one for a fresh generation.
fn retire_after_loss(watched: &Watched, eviction: ProxyEviction, requested_recovery: bool) {
    record_proxy_event(
        &watched.spec,
        watched.pid,
        "proxy-loss-eviction-start",
        "generation-retired",
    );
    let recovery_candidate =
        requested_recovery && !eviction.final_tenant && legacy_lock(&watched.directory).is_ok();
    if requested_recovery && !recovery_candidate {
        record_proxy_event(
            &watched.spec,
            watched.pid,
            "proxy-recovery-skipped",
            "no-recovery",
        );
    }
    futures::executor::block_on(eviction.completed);
    record_proxy_event(
        &watched.spec,
        watched.pid,
        "proxy-loss-eviction-complete",
        "writers-drained",
    );
    {
        let mut child = watched
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let _ignored = child.0.kill();
        let _ignored = child.0.wait();
    }
    // The registry keeps this proxy's entry, marked lost, so that a registration of the
    // retired generation arriving late fails instead of launching a proxy for it.
    if !(recovery_candidate && eviction.recovery_allowed.load(Ordering::Acquire)) {
        clear_slot_if_owned(watched);
        return;
    }
    {
        // Hold the lock only for the check that the slot still names this proxy, not
        // across the replacement's launch.
        let Ok(_lifecycle) = legacy_lock(&watched.directory) else {
            return;
        };
        if !slot_matches(&watched.spec, &watched.directory, watched.pid) {
            return;
        }
        // Taken last, so that a recovery ruled out above does not count against the budget.
        if !take_recovery_budget(&watched.key) {
            record_proxy_event(
                &watched.spec,
                watched.pid,
                "proxy-recovery-skipped",
                "budget-exhausted",
            );
            remove_slot_files(watched);
            return;
        }
    }
    recover(watched);
}

/// Leaves the slot as a stopped daemon leaves it, empty rather than naming a dead process, when
/// nothing replaces the lost proxy and the slot still names it.
fn clear_slot_if_owned(watched: &Watched) {
    if let Ok(_lifecycle) = legacy_lock(&watched.directory)
        && slot_matches(&watched.spec, &watched.directory, watched.pid)
    {
        remove_slot_files(watched);
    }
}

fn remove_slot_files(watched: &Watched) {
    for path in slot_files(&watched.directory) {
        let _ignored = std::fs::remove_file(path);
    }
}

/// Replaces the lost proxy's tenant with a fresh generation and a proxy for it.
fn recover(watched: &Watched) {
    match futures::executor::block_on((watched.supervisor.register_replacement)(
        watched.spec.clone(),
    )) {
        Ok(Some(info)) => {
            record_proxy_event(&watched.spec, info.pid, "proxy-respawn", "fresh-generation")
        }
        Ok(None) => {}
        Err(_) => record_proxy_event(
            &watched.spec,
            watched.pid,
            "proxy-recovery-failed",
            "registration-failed",
        ),
    }
}

fn proxy_was_killed(child: &Mutex<ProxyChild>) -> bool {
    use std::os::unix::process::ExitStatusExt;
    let deadline = Instant::now() + Duration::from_millis(250);
    loop {
        let status = child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .0
            .try_wait();
        match status {
            Ok(Some(status)) => {
                return matches!(status.signal(), Some(libc::SIGKILL | libc::SIGTERM));
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            _ => return false,
        }
    }
}
