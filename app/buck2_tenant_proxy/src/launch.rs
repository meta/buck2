/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Launching the proxy of a tenant generation from the shared daemon.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::Instant;

use buck2_cli_proto::DaemonProcessInfo;
use buck2_common::daemon_compatibility::TENANT_PROXY_SPEC_FILE;
use buck2_common::daemon_compatibility::TenantProxySpec;
use buck2_common::daemon_dir::DaemonDir;
use buck2_error::ErrorTag;
use buck2_error::buck2_error;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::file_name::FileName;
use buck2_server::daemon::proxies::ProxySupervisor;
use buck2_wrapper_common::kill::TENANT_PROXY_ARG0;
use buck2_wrapper_common::kill::TENANT_PROXY_FILE_FLAG;
use futures::future::BoxFuture;

use crate::registry;
use crate::registry::proxy_key;
use crate::slot::proxy_dir;
use crate::supervisor;

/// How long the launcher waits for the proxy to publish, which covers the proxy waiting out a
/// client that holds the slot's lifecycle lock across a legacy kill.
pub(crate) const PROXY_STARTUP_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) struct ProxyChild(pub(crate) std::process::Child);

impl Drop for ProxyChild {
    fn drop(&mut self) {
        let _ignored = self.0.kill();
        let _ignored = self.0.wait();
    }
}

pub fn launch(
    spec: TenantProxySpec,
    supervisor: ProxySupervisor,
) -> BoxFuture<'static, buck2_error::Result<DaemonProcessInfo>> {
    Box::pin(
        async move { tokio::task::spawn_blocking(move || launch_sync(spec, supervisor)).await? },
    )
}

struct ProxyStartup {
    child: Arc<Mutex<ProxyChild>>,
    committed: bool,
}

impl Drop for ProxyStartup {
    fn drop(&mut self) {
        if !self.committed {
            let mut child = self.child.lock().unwrap_or_else(|error| error.into_inner());
            let _ignored = child.0.kill();
            let _ignored = child.0.wait();
        }
    }
}

/// Launches the proxy of `spec`'s generation and waits until it has published itself in the
/// tenant's own daemon directory. Returns the connection record it published.
pub(crate) fn launch_sync(
    spec: TenantProxySpec,
    supervisor: ProxySupervisor,
) -> buck2_error::Result<DaemonProcessInfo> {
    let key = proxy_key(&spec);
    let (lost, exit, mut reservation) = match registry::reserve(&key)? {
        registry::Reservation::Published(info) => return Ok(info),
        registry::Reservation::Reserved {
            lost,
            exit,
            reservation,
        } => (lost, exit, reservation),
    };
    let directory = proxy_dir(&spec)?;
    std::fs::create_dir_all(directory.path.as_path())?;
    let spec_path = write_spec(&directory, &spec)?;
    let launched = launch_spawned(
        &spec,
        &spec_path,
        &directory,
        key,
        lost,
        exit,
        &mut reservation,
        supervisor,
    );
    if launched.is_err() {
        remove_spec_if_own(&spec_path, &spec);
    }
    launched
}

/// Removes the spec file written for `spec` unless another launch has replaced it. A spec that
/// outlives its failed launch would leave the daemon's credentials in a directory the launch
/// never came to own.
fn remove_spec_if_own(spec_path: &AbsNormPathBuf, spec: &TenantProxySpec) {
    let current = File::open(spec_path)
        .ok()
        .and_then(|file| serde_json::from_reader::<_, TenantProxySpec>(file).ok());
    if current.is_some_and(|current| current.reference == spec.reference) {
        let _ignored = std::fs::remove_file(spec_path);
    }
}

fn launch_spawned(
    spec: &TenantProxySpec,
    spec_path: &AbsNormPathBuf,
    directory: &DaemonDir,
    key: registry::ProxyKey,
    lost: Arc<AtomicBool>,
    exit: Arc<registry::ProxyExit>,
    reservation: &mut registry::ProxyReservation,
    supervisor: ProxySupervisor,
) -> buck2_error::Result<DaemonProcessInfo> {
    let (socket, child_socket) = UnixStream::pair()?;
    let retire = socket.try_clone()?;
    let child = Arc::new(Mutex::new(ProxyChild(spawn(
        spec,
        spec_path,
        &child_socket,
    )?)));
    drop(child_socket);
    let mut guard = ProxyStartup {
        child: child.clone(),
        committed: false,
    };
    let pid = child
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .0
        .id() as i64;
    if registry::spawned(&key, pid, retire) {
        // The guard ends the process; a record it managed to publish names a dead process,
        // which clients treat as a vacant slot. The reservation marks the entry lost, so a late
        // registration of the generation fails rather than relaunching.
        return Err(buck2_error!(
            ErrorTag::Input,
            "Tenant generation was retired before its proxy started"
        ));
    }
    supervisor::watch(
        socket,
        supervisor::Watched {
            child: child.clone(),
            spec: spec.clone(),
            directory: directory.clone(),
            key: key.clone(),
            pid,
            lost: lost.clone(),
            exit,
            supervisor,
        },
    )?;
    let info = wait_for_publication(directory, pid, &guard, &lost)?;
    registry::record_info(&key, &info);
    if lost.load(Ordering::Acquire) {
        return Err(buck2_error!(
            ErrorTag::Input,
            "Proxy liveness was lost during publication"
        ));
    }
    guard.committed = true;
    reservation.committed = true;
    registry::mark_published(&key);
    Ok(info)
}

/// Writes the proxy's spec, which carries the shared daemon's credentials, to a temporary name
/// and renames it into place, so that a reader never sees a partial file.
fn write_spec(
    directory: &DaemonDir,
    spec: &TenantProxySpec,
) -> buck2_error::Result<AbsNormPathBuf> {
    let spec_path = directory
        .path
        .join(FileName::unchecked_new(TENANT_PROXY_SPEC_FILE));
    let staging = directory.path.join(FileName::new(&format!(
        "{TENANT_PROXY_SPEC_FILE}.{:016x}",
        rand::random::<u64>()
    ))?);
    let written = (|| -> buck2_error::Result<()> {
        let mut file = File::create(&staging)?;
        serde_json::to_writer(&mut file, spec)?;
        file.sync_all()?;
        std::fs::rename(&staging, &spec_path)?;
        Ok(())
    })();
    if let Err(error) = written {
        let _ignored = std::fs::remove_file(&staging);
        return Err(error);
    }
    Ok(spec_path)
}

/// Starts the proxy process with its end of the liveness socket inherited.
fn spawn(
    spec: &TenantProxySpec,
    spec_path: &AbsNormPathBuf,
    child_socket: &UnixStream,
) -> buck2_error::Result<std::process::Child> {
    let startup = spec
        .constraints
        .daemon_startup_config
        .as_deref()
        .ok_or_else(|| buck2_error!(ErrorTag::Input, "Proxy lacks startup configuration"))?;
    let descriptor = child_socket.as_raw_fd();
    let mut command = buck2_util::process::background_command(std::env::current_exe()?);
    command
        .current_dir(&spec.tenant.project_root)
        .args([
            "--isolation-dir",
            &spec.tenant.isolation,
            "daemon",
            "--dont-daemonize",
            "--skip-macos-qos",
            "--daemon-id",
            &spec.reference.daemon_id,
        ])
        .arg(format!("--{TENANT_PROXY_FILE_FLAG}"))
        .arg(spec_path.as_path())
        .arg("--tenant-proxy-fd")
        .arg(descriptor.to_string())
        .arg(startup)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        command.arg0(TENANT_PROXY_ARG0);
        // SAFETY: pre_exec only changes the inheritance flag of the owned socket descriptor.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(descriptor, libc::F_GETFD);
                if flags < 0
                    || libc::fcntl(descriptor, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    Ok(command.spawn()?)
}

/// Waits until the proxy has published a connection record naming itself, or has failed.
fn wait_for_publication(
    directory: &DaemonDir,
    pid: i64,
    startup: &ProxyStartup,
    lost: &AtomicBool,
) -> buck2_error::Result<DaemonProcessInfo> {
    let deadline = Instant::now() + PROXY_STARTUP_TIMEOUT;
    loop {
        if lost.load(Ordering::Acquire) {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Proxy liveness was lost during startup"
            ));
        }
        if let Ok(file) = File::open(directory.buckd_info()) {
            if let Ok(info) = serde_json::from_reader::<_, DaemonProcessInfo>(file) {
                if info.pid == pid {
                    return Ok(info);
                }
            }
        }
        if let Some(status) = startup
            .child
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .0
            .try_wait()?
        {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Tenant proxy startup failed ({status}): {}",
                std::fs::read_to_string(directory.buckd_stderr()).unwrap_or_default()
            ));
        }
        if Instant::now() >= deadline {
            return Err(buck2_error!(
                ErrorTag::Input,
                "Timed out starting tenant proxy"
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
