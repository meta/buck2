/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Finding and killing every descendant of a process, including ones that left its process group
//! or session. Without a cgroup to kill, this is the only way to stop a test's detached daemons.
//!
//! A process that double-forks normally ends up a child of init, so it no longer shows up as a
//! descendant. Making the root process a child subreaper (`PR_SET_CHILD_SUBREAPER`) keeps such
//! processes in its subtree for as long as the root lives.

use std::collections::VecDeque;
use std::fs;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::fd::RawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;

use buck2_hash::BuckMutMap;
use nix::errno::Errno;
use nix::sys::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;

/// Makes the spawned process a child subreaper, so that its detached descendants stay its
/// descendants instead of being reparented to init.
///
/// It then adopts its orphaned descendants, and it does not reap them unless it waits for all its
/// children: they stay zombies until it exits. A test waiting for one of its daemons to exit by
/// polling its pid then waits until it is killed, and a daemon that watches for its parent dying
/// through `getppid() == 1` does not see it.
///
/// Best effort: a process that cannot become a subreaper still runs, and its double-forked
/// descendants are not found.
pub(crate) fn adopt_detached_descendants(cmd: &mut Command) {
    // SAFETY: `prctl` is async-signal-safe and does not allocate.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
            Ok(())
        });
    }
}

/// A process, identified by its pid and start time so that a reused pid is never mistaken for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessId {
    pid: i32,
    start_time: u64,
}

impl ProcessId {
    #[cfg(test)]
    pub(crate) fn pid(&self) -> i32 {
        self.pid
    }

    fn is_running(&self) -> bool {
        read_stat(self.pid).is_some_and(|(_, start_time)| start_time == self.start_time)
    }

    /// Sends `SIGKILL` to the process if it is still running, and is still the same process.
    ///
    /// Signals it through a pidfd, which refers to one process for good: once the start time
    /// checked after opening it matches, the signal cannot reach a process that reused the pid.
    pub(crate) fn kill(&self) {
        // SAFETY: `pidfd_open` takes no pointers.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.pid, 0) };
        if fd < 0 {
            match Errno::last() {
                Errno::ESRCH => {}
                // Kernels before 5.3: a pid reused between the check and the signal gets it.
                Errno::ENOSYS if self.is_running() => {
                    match signal::kill(Pid::from_raw(self.pid), Signal::SIGKILL) {
                        Ok(()) | Err(Errno::ESRCH) => {}
                        Err(e) => self.warn(e),
                    }
                }
                Errno::ENOSYS => {}
                e => self.warn(e),
            }
            return;
        }
        // SAFETY: `pidfd_open` returned a new file descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(fd as RawFd) };
        if !self.is_running() {
            return;
        }
        // SAFETY: `fd` is a pidfd; a null `siginfo` is allowed and sends a plain `SIGKILL`.
        let res = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if res != 0 && Errno::last() != Errno::ESRCH {
            self.warn(Errno::last());
        }
    }

    fn warn(&self, e: Errno) {
        tracing::warn!("Failed to kill detached descendant {}: {}", self.pid, e);
    }
}

/// `(ppid, start_time)` from `/proc/<pid>/stat`. The command name may contain anything, so the
/// fields are read after its closing `)`.
fn parse_stat(stat: &str) -> Option<(i32, u64)> {
    let (_, rest) = stat.rsplit_once(')')?;
    // Fields after the command name start at field 3 (`state`); `ppid` is field 4 and
    // `starttime` is field 22.
    let mut fields = rest.split_whitespace();
    let ppid = fields.nth(1)?.parse().ok()?;
    let start_time = fields.nth(17)?.parse().ok()?;
    Some((ppid, start_time))
}

fn read_stat(pid: i32) -> Option<(i32, u64)> {
    parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// All live descendants of `root`, not including `root` itself.
pub(crate) fn descendants(root: i32) -> Vec<ProcessId> {
    let mut children = children_by_parent();
    walk(&mut children, vec![root])
}

/// All live descendants of `leader` and of those of `roots` that are still running, not including
/// them. `leader` must be a child of this process that it has not reaped, so that its pid is still
/// its own.
pub(crate) fn descendants_of(leader: Option<i32>, roots: &[ProcessId]) -> Vec<ProcessId> {
    let mut children = children_by_parent();
    let roots = leader
        .into_iter()
        .chain(roots.iter().filter(|r| r.is_running()).map(|r| r.pid))
        .collect();
    walk(&mut children, roots)
}

fn children_by_parent() -> BuckMutMap<i32, Vec<ProcessId>> {
    let mut children: BuckMutMap<i32, Vec<ProcessId>> = BuckMutMap::default();
    let Ok(entries) = fs::read_dir("/proc") else {
        return children;
    };
    for pid in entries.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok()) {
        if let Some((ppid, start_time)) = read_stat(pid) {
            children
                .entry(ppid)
                .or_default()
                .push(ProcessId { pid, start_time });
        }
    }
    children
}

fn walk(children: &mut BuckMutMap<i32, Vec<ProcessId>>, roots: Vec<i32>) -> Vec<ProcessId> {
    let mut found = Vec::new();
    let mut queue = VecDeque::from(roots);
    while let Some(parent) = queue.pop_front() {
        for child in children.remove(&parent).unwrap_or_default() {
            queue.push_back(child.pid);
            found.push(child);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stat() {
        let stat =
            "1234 (a) b (c)) S 42 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 98765 0 0";
        assert_eq!(parse_stat(stat), Some((42, 98765)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn test_descendants_of_self_include_a_child() -> std::io::Result<()> {
        // ast-grep-ignore: rust/buck2-no-command-new
        let mut child = Command::new("sleep").arg("1000").spawn()?;
        let found = descendants(std::process::id() as i32);
        let child_pid = child.id() as i32;
        child.kill()?;
        child.wait()?;
        assert!(
            found.iter().any(|p| p.pid == child_pid),
            "a direct child should be a descendant"
        );
        Ok(())
    }
}
