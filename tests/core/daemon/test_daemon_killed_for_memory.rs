/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! When macOS memory management kills the daemon, the client says so.
//!
//! macOS only: there a test can start a process under a memory limit that the kernel enforces
//! with a memory kill, without any privilege.

use std::ffi::CString;
use std::ffi::OsStr;
use std::mem;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::ptr;
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::prelude::*;
use serde_json::Value;

/// Ten times what a daemon takes to start and to list `//:`, and half of what loading `//hog:`
/// takes.
const MEMORY_LIMIT_MB: libc::c_int = 256;

const POSIX_SPAWN_JETSAM_MEMLIMIT_ACTIVE_FATAL: libc::c_short = 0x04;
const POSIX_SPAWN_JETSAM_MEMLIMIT_INACTIVE_FATAL: libc::c_short = 0x08;

type SetJetsam = unsafe extern "C" fn(
    attr: *mut libc::posix_spawnattr_t,
    flags: libc::c_short,
    priority: libc::c_int,
    memlimit_active: libc::c_int,
    memlimit_inactive: libc::c_int,
) -> libc::c_int;

/// What launchd gives a job its memory limit with. libSystem exports it, but the SDK has no
/// header for it; XNU declares it in libsyscall's spawn_private.h.
fn posix_spawnattr_setjetsam_ext() -> anyhow::Result<SetJetsam> {
    // SAFETY: The name is a NUL-terminated string.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"posix_spawnattr_setjetsam_ext".as_ptr(),
        )
    };
    anyhow::ensure!(
        !symbol.is_null(),
        "This macOS has no `posix_spawnattr_setjetsam_ext` to limit the daemon's memory with"
    );
    // SAFETY: The symbol is the function that XNU declares with this signature.
    Ok(unsafe { mem::transmute::<*mut libc::c_void, SetJetsam>(symbol) })
}

fn c_string(s: impl AsRef<OsStr>) -> anyhow::Result<CString> {
    Ok(CString::new(s.as_ref().as_bytes())?)
}

fn null_terminated(strings: &[CString]) -> Vec<*mut libc::c_char> {
    strings
        .iter()
        .map(|s| s.as_ptr().cast_mut())
        .chain([ptr::null_mut()])
        .collect()
}

async fn status(buck: &Buck) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&buck.status(&[]).await?.stdout)?)
}

/// A daemon that the kernel kills when it uses more memory than `MEMORY_LIMIT_MB`. The test
/// kills it if it is still running when the test is done with it.
struct MemoryLimitedDaemon {
    pid: libc::pid_t,
    /// The wait status, once the daemon has exited and been reaped.
    exited: Option<libc::c_int>,
}

impl MemoryLimitedDaemon {
    /// Starts the daemon for `buck`'s project.
    ///
    /// A client cannot start this daemon: only the process that spawns another can give it a
    /// memory limit, and the limit does not survive an `exec`.
    fn spawn(buck: &Buck, startup_config: &str) -> anyhow::Result<Self> {
        let mut argv = vec![c_string(buck.executable())?];
        if let Some(prefix) = buck.isolation_prefix() {
            argv.extend([c_string("--isolation-dir")?, c_string(prefix)?]);
        }
        for arg in [
            "daemon",
            "--dont-daemonize",
            // Without this the daemon replaces itself with a copy of lower priority, which has
            // no limit.
            "--skip-macos-qos",
            "--daemon-id",
            &uuid::Uuid::new_v4().to_string(),
            startup_config,
        ] {
            argv.push(c_string(arg)?);
        }
        let env = buck
            .envs()
            .map(|(key, value)| {
                let mut entry = key.to_owned();
                entry.push("=");
                entry.push(value);
                c_string(entry)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let cwd = c_string(buck.cwd())?;
        let argv_ptrs = null_terminated(&argv);
        let env_ptrs = null_terminated(&env);
        let set_jetsam = posix_spawnattr_setjetsam_ext()?;

        let mut pid = 0;
        // SAFETY: The attributes and file actions are initialized before use and destroyed
        // after, and every pointer is to a NUL-terminated string or a null-terminated array
        // that outlives the calls.
        let spawned = unsafe {
            let mut attr = MaybeUninit::uninit();
            assert_eq!(libc::posix_spawnattr_init(attr.as_mut_ptr()), 0);
            let mut attr = attr.assume_init();
            assert_eq!(
                set_jetsam(
                    &mut attr,
                    POSIX_SPAWN_JETSAM_MEMLIMIT_ACTIVE_FATAL
                        | POSIX_SPAWN_JETSAM_MEMLIMIT_INACTIVE_FATAL,
                    // The default priority band.
                    -1,
                    MEMORY_LIMIT_MB,
                    MEMORY_LIMIT_MB,
                ),
                0
            );
            let mut file_actions = MaybeUninit::uninit();
            assert_eq!(
                libc::posix_spawn_file_actions_init(file_actions.as_mut_ptr()),
                0
            );
            let mut file_actions = file_actions.assume_init();
            // The daemon finds its project from the directory it starts in.
            assert_eq!(
                libc::posix_spawn_file_actions_addchdir_np(&mut file_actions, cwd.as_ptr()),
                0
            );
            let spawned = libc::posix_spawn(
                &mut pid,
                argv[0].as_ptr(),
                &file_actions,
                &attr,
                argv_ptrs.as_ptr(),
                env_ptrs.as_ptr(),
            );
            libc::posix_spawn_file_actions_destroy(&mut file_actions);
            libc::posix_spawnattr_destroy(&mut attr);
            spawned
        };
        anyhow::ensure!(
            spawned == 0,
            "Could not start the daemon: {}",
            std::io::Error::from_raw_os_error(spawned)
        );
        Ok(Self { pid, exited: None })
    }

    /// The wait status if the daemon has exited, without blocking.
    fn try_wait(&mut self) -> Option<libc::c_int> {
        if self.exited.is_none() {
            let mut wait_status = 0;
            // SAFETY: `pid` is a child of this process that has not been waited for, and
            // `wait_status` is valid for the write.
            if unsafe { libc::waitpid(self.pid, &mut wait_status, libc::WNOHANG) } == self.pid {
                self.exited = Some(wait_status);
            }
        }
        self.exited
    }

    /// Waits until this daemon is the one serving `buck`'s project.
    async fn wait_until_serving(&mut self, buck: &Buck) -> anyhow::Result<()> {
        let pid = self.pid;
        let serving = async {
            loop {
                if let Some(wait_status) = self.try_wait() {
                    anyhow::bail!("The daemon exited as it started, wait status {wait_status:#x}");
                }
                // There is no JSON until a daemon is up.
                if let Ok(status) = status(buck).await
                    && status["process_info"]["pid"] == pid
                {
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(60), serving)
            .await
            .with_context(|| format!("The daemon {pid} did not come up"))?
    }

    /// Waits for the daemon to exit, which it is expected to be doing, and returns its wait
    /// status.
    async fn wait(&mut self) -> anyhow::Result<libc::c_int> {
        for _ in 0..300 {
            if let Some(wait_status) = self.try_wait() {
                return Ok(wait_status);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        anyhow::bail!("The daemon {} did not exit", self.pid)
    }
}

impl Drop for MemoryLimitedDaemon {
    fn drop(&mut self) {
        if self.try_wait().is_none() {
            // SAFETY: Signals and reaps a child of this process that has not been waited for.
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                libc::waitpid(self.pid, ptr::null_mut(), 0);
            }
        }
    }
}

#[buck_test]
async fn test_client_reports_daemon_killed_over_its_memory_limit(
    buck: &Buck,
) -> anyhow::Result<()> {
    // Clients use a running daemon only if they would start one the same way, so start ours
    // the way a client starts its own.
    buck.targets(&["//:"]).await?;
    let startup_config = status(buck).await?["daemon_constraints"]["daemon_startup_config"]
        .as_str()
        .context("No `daemon_startup_config` in the daemon's status")?
        .to_owned();
    buck.kill().await?;

    let mut daemon = MemoryLimitedDaemon::spawn(buck, &startup_config)?;
    daemon.wait_until_serving(buck).await?;

    let output = buck.targets(&["//hog:"]).await.expect_failure();
    output.assert_exit_code(ExitCode::FatalOom);
    output.assert_stderr_matches(&format!(
        r"macOS memory management killed daemon PID {}: memory limit exceeded",
        daemon.pid
    ));
    let wait_status = daemon.wait().await?;
    assert!(
        libc::WIFSIGNALED(wait_status) && libc::WTERMSIG(wait_status) == libc::SIGKILL,
        "The kernel should have killed the daemon, wait status {wait_status:#x}"
    );
    Ok(())
}
