/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The proxy process: started by the shared daemon for one tenant generation, it publishes the
//! connection files of the tenant's own daemon directory and serves the daemon API there until the
//! daemon retires the generation or goes away.

use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use buck2_cli_proto::DaemonProcessInfo;
use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_common::daemon_dir::DaemonDir;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_server::daemon::daemon_tcp::create_listener;
use buck2_server::daemon::proxies::proxy_close_liveness_gate;
use buck2_util::threads::thread_spawn;
use buck2_wrapper_common::kill;

use crate::server::ProxyService;
use crate::server::ProxyState;
use crate::slot::live_slot_owner;
use crate::slot::lock_legacy_slot;
use crate::slot::proxy_dir;

/// Whether the shared daemon's process is gone, counting a zombie as gone: the daemon's parent
/// may not have reaped it yet, and its outputs are released either way.
pub(crate) fn daemon_exited(pid: i64) -> bool {
    // Read from procfs directly rather than through `sysinfo`, which scans all of `/proc` and
    // takes about a second on a busy host: this is polled every few milliseconds while an
    // eviction completes and inside the 400 ms forced tree kill.
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(')')
            .is_some_and(|(_, fields)| matches!(fields.split_whitespace().next(), Some("Z" | "X"))),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

fn write_process_info(directory: &DaemonDir, info: &DaemonProcessInfo) -> buck2_error::Result<()> {
    let file = File::create(directory.buckd_info())?;
    serde_json::to_writer(&file, info)?;
    // Fsync so the endpoint and auth token are durable before clients race to read this file.
    file.sync_all()?;
    Ok(())
}

fn redirect_output(stdout: &File, stderr: &File) -> buck2_error::Result<()> {
    nix::unistd::dup2_stdout(stdout)?;
    nix::unistd::dup2_stderr(stderr)?;
    Ok(())
}

fn append_log(path: AbsNormPathBuf) -> buck2_error::Result<File> {
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?)
}

/// How long a proxy waits for its tenant directory's lifecycle lock before giving up. A client
/// holds it across a kill of the previous owner, which may take the legacy four-second grace.
const SLOT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(path: &Path, descriptor: Option<i32>) -> buck2_error::Result<()> {
    let spec: TenantProxySpec = serde_json::from_reader(File::open(path)?)?;
    let descriptor = descriptor
        .ok_or_else(|| buck2_error!(ErrorTag::Input, "Missing proxy liveness descriptor"))?;
    // SAFETY: `descriptor` is the proxy's end of the `AF_UNIX` stream socketpair the launcher
    // created for this process and named with `--tenant-proxy-fd`; the launcher closed its copy
    // after spawning, nothing else in this process refers to the descriptor, and it is wrapped
    // exactly once here, so the `UnixStream` is its sole owner.
    let mut monitor = unsafe { UnixStream::from_raw_fd(descriptor) };
    // SAFETY: `F_SETFD` only sets the close-on-exec flag on a descriptor this process owns; it
    // reads no memory and the flag has no bearing on the `UnixStream` above. Without it a
    // process the proxy spawns would inherit the socket and keep the daemon's liveness view of
    // this proxy alive after the proxy exits.
    let result = unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) };
    if result < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut reference = serde_json::to_vec(&spec.reference)?;
    reference.push(b'\n');
    monitor.write_all(&reference)?;
    let mut acknowledgement = [0];
    let retirement = monitor.try_clone()?;
    monitor.read_exact(&mut acknowledgement)?;
    if acknowledgement != *b"A" {
        return Err(buck2_error!(
            ErrorTag::Input,
            "Invalid proxy liveness acknowledgement"
        ));
    }
    let directory = proxy_dir(&spec)?;
    // Appended to until this process owns the directory: the logs may still be a live owner's.
    let stdout = append_log(directory.buckd_stdout())?;
    let stderr = append_log(directory.buckd_stderr())?;
    redirect_output(&stdout, &stderr)?;
    let (endpoint, listener) = create_listener()?;
    listener.set_nonblocking(true)?;
    let info = DaemonProcessInfo {
        pid: std::process::id() as i64,
        endpoint: endpoint.to_string(),
        version: spec.daemon.version.clone(),
        auth_token: format!("{:032x}", rand::random::<u128>()),
        pid_start_time: kill::process_start_time(None).unwrap_or(0),
    };
    {
        // Clients of every release change this directory's owner only under its lifecycle
        // lock, so publishing under it too means none of them finds a half-written slot or
        // has its own daemon's record overwritten.
        let _lifecycle = lock_legacy_slot(&directory, SLOT_LOCK_TIMEOUT)?;
        if let Some(owner) = live_slot_owner(&directory, info.pid)? {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Tenant directory `{directory}` is owned by live process {owner}; not publishing a proxy over it"
            ));
        }
        std::fs::write(directory.buckd_pid(), info.pid.to_string())?;
        write_process_info(&directory, &info)?;
        stdout.set_len(0)?;
        stderr.set_len(0)?;
    }
    let state = Arc::new(ProxyState {
        spec,
        info,
        directory,
        started: Instant::now(),
        start_time: SystemTime::now(),
        kill_started: AtomicBool::new(false),
        force_started: AtomicBool::new(false),
        eviction_started: Mutex::new(None),
        finished: AtomicBool::new(false),
        notify: tokio::sync::Notify::new(),
        monitor: Mutex::new(monitor),
    });
    let runtime = buck2_util::tokio_runtime::new_tokio_runtime("buck2-proxy")
        .enable_all()
        .worker_threads(2)
        .build()?;
    runtime.block_on(async {
        let listener = tokio_stream::wrappers::TcpListenerStream::new(
            tokio::net::TcpListener::from_std(listener)?,
        );
        let retiring = state.clone();
        let _ignored = thread_spawn("buck2-proxy-retirement", move || {
            // The daemon writes once this generation's eviction has completed. The socket
            // closing means nothing here: the daemon's exit is noticed below by its pid, and
            // a proxy that shut its own end keeps serving until the daemon retires it.
            let mut message = [0];
            loop {
                match (&retirement).read(&mut message) {
                    Ok(_) if message[0] == b'E' => break,
                    Ok(0) => return,
                    Ok(_) => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return,
                }
            }
            if !retiring.finished.load(Ordering::Acquire) {
                retiring.record("daemon-retired", "generation-evicted");
                retiring.finish();
            }
        });
        let parent = state.clone();
        let close_liveness_gate = proxy_close_liveness_gate()?
            .and_then(|gate| gate.split_once(','))
            .filter(|(isolation, _)| *isolation == parent.spec.tenant.isolation)
            .map(|(_, path)| PathBuf::from(path));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if parent.finished.load(Ordering::Acquire) {
                    break;
                }
                if let Some(gate) = &close_liveness_gate
                    && tokio::fs::try_exists(gate).await.unwrap_or(false)
                {
                    let _ignored = parent
                        .monitor
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .shutdown(std::net::Shutdown::Both);
                }
                if daemon_exited(parent.spec.daemon.pid) {
                    parent.record(
                        "daemon-exit",
                        if parent.kill_started.load(Ordering::Acquire) {
                            "during-eviction"
                        } else {
                            "outside-eviction"
                        },
                    );
                    parent.finish();
                    break;
                }
                if parent.finished.load(Ordering::Acquire) {
                    break;
                }
            }
        });
        let server = tonic::transport::Server::builder()
            .add_service(ProxyService(state.clone()))
            .serve_with_incoming(listener);
        tokio::select! {
            result = server => result?,
            _ = state.notify.notified() => { tokio::time::sleep(Duration::from_millis(20)).await; }
        }
        Ok::<_, buck2_error::Error>(())
    })?;
    state.cleanup();
    Ok(())
}
