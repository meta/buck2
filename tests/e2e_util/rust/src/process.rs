/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Watching and signalling processes by id: the daemon, and the processes of started commands.

use std::time::Duration;
use std::time::Instant;

#[cfg(unix)]
use anyhow::Context;
use serde_json::Value;

use crate::Buck;
#[cfg(unix)]
use crate::BuckError;
#[cfg(unix)]
use crate::BuckOutput;
#[cfg(unix)]
use crate::BuckProcess;

/// How often the waits here check again.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Whether a process with the id `pid` exists, as `psutil.pid_exists` answers it: a process the
/// test may not signal exists, and on Unix so does one that has exited but is not yet reaped. No
/// process has the id 0.
pub fn daemon_is_alive(pid: u32) -> bool {
    pid != 0 && imp::process_exists(pid)
}

/// Waits up to `timeout` for the process `pid` to be gone; whether it is.
pub async fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while daemon_is_alive(pid) {
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    true
}

/// The pid of the daemon, as `buck2 status` reports it, waiting up to `timeout` for one to run:
/// for a daemon a started command brings up.
pub async fn wait_for_daemon_pid(buck: &Buck, timeout: Duration) -> anyhow::Result<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let status = buck.status(&[]).await?;
        if let Some(pid) = pid_from_status(&status.stdout) {
            return Ok(pid);
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "No daemon is running after {timeout:?}; the last status said:\n{}{}",
            status.stdout,
            status.stderr
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn pid_from_status(stdout: &str) -> Option<u32> {
    let status: Value = serde_json::from_str(stdout).ok()?;
    u32::try_from(status["process_info"]["pid"].as_u64()?).ok()
}

/// A signal a test sends.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// `SIGINT`, as Ctrl-C sends it.
    Interrupt,
    /// `SIGTERM`.
    Terminate,
    /// `SIGKILL`.
    Kill,
}

#[cfg(unix)]
impl Signal {
    fn number(self) -> libc::c_int {
        match self {
            Self::Interrupt => libc::SIGINT,
            Self::Terminate => libc::SIGTERM,
            Self::Kill => libc::SIGKILL,
        }
    }
}

/// Sends `signal` to the process `pid`.
#[cfg(unix)]
pub fn send_signal(pid: u32, signal: Signal) -> anyhow::Result<()> {
    // `kill` takes 0 to mean the caller's process group.
    anyhow::ensure!(pid != 0, "`0` is not a pid");
    let raw = libc::pid_t::try_from(pid).with_context(|| format!("`{pid}` is not a pid"))?;
    // SAFETY: `kill` has no memory safety requirements.
    if unsafe { libc::kill(raw, signal.number()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
            .with_context(|| format!("Sending {signal:?} to process {pid}"))
    }
}

#[cfg(unix)]
impl BuckProcess {
    /// Sends `signal` to the process, which must not have been waited for.
    pub fn send_signal(&self, signal: Signal) -> anyhow::Result<()> {
        let pid = self.id().context("The process has been waited for")?;
        send_signal(pid, signal)
    }

    /// Sends `SIGINT`, as Ctrl-C does, then waits for the process like [`BuckProcess::wait`].
    pub async fn interrupt(self) -> Result<BuckOutput, BuckError> {
        self.send_signal(Signal::Interrupt)
            .unwrap_or_else(|e| panic!("{e:#}"));
        self.wait().await
    }
}

#[cfg(unix)]
mod imp {
    pub(super) fn process_exists(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: `kill` has no memory safety requirements; signal 0 only checks for the process.
        let found = unsafe { libc::kill(pid, 0) } == 0;
        found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Foundation::STILL_ACTIVE;
    use windows_sys::Win32::System::Threading::GetExitCodeProcess;
    use windows_sys::Win32::System::Threading::OpenProcess;
    use windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION;

    pub(super) fn process_exists(pid: u32) -> bool {
        // SAFETY: `OpenProcess` and `GetLastError` take no pointers; after the null check `handle` is
        // a valid process handle owned here, `&mut exit_code` is a live `u32` for
        // `GetExitCodeProcess`, and `CloseHandle` closes `handle` once, after its last use.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return GetLastError() == ERROR_ACCESS_DENIED;
            }
            let mut exit_code = 0;
            let running =
                GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE as u32;
            CloseHandle(handle);
            running
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pid of a process that has exited and been reaped.
    fn exited_pid() -> u32 {
        let mut command = if cfg!(windows) {
            let mut command = std::process::Command::new("cmd");
            command.args(["/c", "exit 0"]);
            command
        } else {
            std::process::Command::new("true")
        };
        let mut child = command.spawn().unwrap();
        let pid = child.id();
        assert!(child.wait().unwrap().success());
        pid
    }

    #[test]
    fn test_this_process_is_alive() {
        assert!(daemon_is_alive(std::process::id()));
    }

    #[test]
    fn test_a_reaped_process_is_not_alive() {
        assert!(!daemon_is_alive(exited_pid()));
    }

    #[test]
    fn test_a_pid_out_of_range_is_not_alive() {
        assert!(!daemon_is_alive(u32::MAX));
        assert!(!daemon_is_alive(0), "0 is not a pid");
    }

    #[tokio::test]
    async fn test_waiting_for_exit() {
        assert!(wait_for_exit(exited_pid(), Duration::from_secs(10)).await);
        assert!(
            !wait_for_exit(std::process::id(), Duration::from_millis(50)).await,
            "this process is still running"
        );
    }

    #[test]
    fn test_the_daemon_pid_comes_from_the_status_json() {
        assert_eq!(
            pid_from_status(r#"{"process_info": {"pid": 42}, "uptime": 1}"#),
            Some(42)
        );
        assert_eq!(pid_from_status(""), None, "no daemon prints no status");
        assert_eq!(pid_from_status(r#"{"process_info": {}}"#), None);
    }

    #[cfg(unix)]
    fn sleeper() -> Buck {
        Buck::new(
            "/bin/sh".into(),
            std::env::temp_dir(),
            [("PATH".into(), std::env::var_os("PATH").unwrap_or_default())].into(),
            false,
        )
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_interrupting_a_process_stops_it_with_sigint() {
        use std::os::unix::process::ExitStatusExt;

        let buck = sleeper();
        let process = buck.command(&["-c", "exec sleep 60"]).spawn();
        let output = process
            .interrupt()
            .await
            .expect_err("an interrupted sleep fails");
        assert_eq!(output.status.signal(), Some(libc::SIGINT));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_signals_reach_the_process_by_id() {
        use std::os::unix::process::ExitStatusExt;

        let buck = sleeper();
        let process = buck.command(&["-c", "exec sleep 60"]).spawn();
        send_signal(process.id().unwrap(), Signal::Terminate).unwrap();
        let output = process.wait().await.expect_err("a terminated sleep fails");
        assert_eq!(output.status.signal(), Some(libc::SIGTERM));
        assert!(
            send_signal(0, Signal::Kill).is_err(),
            "0 would signal the test's own process group"
        );
    }
}
