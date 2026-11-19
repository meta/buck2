/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_error::BuckErrorContext;
use nix::sys::signal::Signal;
use sysinfo::Process;

use crate::kill::get_sysinfo_status;
use crate::pid::Pid;

pub(crate) fn process_creation_time(process: &Process) -> Option<Duration> {
    // Returns process creation time with 1 second precision.
    Some(Duration::from_secs(process.start_time()))
}

pub(crate) fn process_exists(pid: Pid) -> buck2_error::Result<bool> {
    Ok(match get_sysinfo_status(pid) {
        // It occasionally happens that systemd on a machine becomes unresponsive and stops reaping
        // its children. Unfortunately, there's not really much that we can do about that, and it
        // does typically eventually resolve itself. In the meantime though, the user may be waiting
        // on the daemon restart to finish to continue their work, so let's not block them.
        Some(sysinfo::ProcessStatus::Zombie) => has_other_tasks(pid),
        Some(_) => true,
        None => false,
    })
}

/// Whether a thread group whose leader has exited still has other tasks. The leader is reported
/// as a zombie as soon as it has exited; the other tasks go on releasing the process's memory and,
/// after that, its file table with its locks.
#[cfg(target_os = "linux")]
fn has_other_tasks(pid: Pid) -> bool {
    std::fs::read_dir(format!("/proc/{}/task", pid.to_u32()))
        .map(|tasks| tasks.filter_map(Result::ok).nth(1).is_some())
        .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
fn has_other_tasks(_pid: Pid) -> bool {
    false
}

pub(crate) fn kill(pid: Pid) -> buck2_error::Result<Option<KilledProcessHandleImpl>> {
    let pid_nix = pid.to_nix()?;

    match nix::sys::signal::kill(pid_nix, Signal::SIGKILL) {
        Ok(()) => Ok(Some(KilledProcessHandleImpl { pid })),
        Err(nix::errno::Errno::ESRCH) => Ok(None),
        Err(e) => Err(e).with_buck_error_context(|| format!("Failed to kill pid {pid}")),
    }
}

pub(crate) struct KilledProcessHandleImpl {
    pid: Pid,
}

impl KilledProcessHandleImpl {
    pub(crate) fn has_exited(&self) -> buck2_error::Result<bool> {
        Ok(!process_exists(self.pid)?)
    }
}

#[cfg(test)]
mod tests {
    use std::thread::sleep;
    use std::time::Duration;

    use crate::background_command;
    use crate::pid::Pid;
    use crate::unix::kill::process_exists;

    #[test]
    fn test_zombie_process_exist() {
        let mut command = background_command("sh");
        command.args(["-c", "exit 0"]);
        let mut child = command.spawn().unwrap();
        // sleep a bit to let the child exit
        sleep(Duration::from_secs(1));
        let pid = Pid::from_u32(child.id()).unwrap();
        // we consider zombie as non existent
        assert!(!process_exists(pid).unwrap(), "process shouldn't exist");
        child.wait().unwrap();
    }

    /// The leader exits first; a thread keeps the process alive.
    #[cfg(target_os = "linux")]
    #[test]
    fn test_zombie_leader_with_live_threads_exists() {
        use nix::libc;

        /// Kills and reaps the child however the test ends.
        struct Child(libc::pid_t);
        impl Drop for Child {
            fn drop(&mut self) {
                // SAFETY: plain libc calls on a pid this process forked and has not reaped.
                unsafe {
                    libc::kill(self.0, libc::SIGKILL);
                    libc::waitpid(self.0, std::ptr::null_mut(), 0);
                }
            }
        }

        let parent = std::process::id();
        // SAFETY: the child only spawns a thread, which glibc's fork handlers keep possible, and
        // leaves through raw exits, never through the test harness's frames.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            // Dies with the test thread, so a killed test binary leaves nothing behind.
            // SAFETY: plain libc calls without pointer arguments.
            let tied = unsafe {
                libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == 0
                    && libc::getppid() as u32 == parent
            };
            let thread = tied.then(|| {
                std::thread::Builder::new().spawn(|| {
                    sleep(Duration::from_secs(60));
                    // SAFETY: `_exit` has no preconditions; it bounds the child's life if the
                    // test did not kill it.
                    unsafe { libc::_exit(0) }
                })
            });
            if !matches!(thread, Some(Ok(_))) {
                // SAFETY: `_exit` has no preconditions, and keeps the child out of the rest of
                // the test binary.
                unsafe { libc::_exit(1) }
            }
            // Ends this thread alone. Not `pthread_exit`: its forced unwind would run through
            // the harness's Rust frames.
            // SAFETY: `exit` takes a status only; the thread spawned above keeps the process.
            unsafe {
                libc::syscall(libc::SYS_exit, 0);
                libc::_exit(1)
            }
        }
        let child = Child(pid);
        let pid = Pid::from_u32(pid as u32).unwrap();
        let state = || {
            std::fs::read_to_string(format!("/proc/{}/stat", pid.to_u32()))
                .ok()
                .and_then(|stat| {
                    stat.rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
                })
        };
        let tasks = || {
            std::fs::read_dir(format!("/proc/{}/task", pid.to_u32()))
                .map(|tasks| tasks.count())
                .unwrap_or(0)
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while state().as_deref() != Some("Z") {
            assert!(
                std::time::Instant::now() < deadline,
                "leader did not exit: {:?}",
                state()
            );
            sleep(Duration::from_millis(5));
        }
        assert_eq!(tasks(), 2, "the leader is a zombie and its thread is alive");
        assert!(
            process_exists(pid).unwrap(),
            "a zombie leader with a live thread is still a process"
        );
        drop(child);
        assert!(!process_exists(pid).unwrap(), "process shouldn't exist");
    }
}
