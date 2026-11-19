/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Finding every descendant of a process, including ones that left its process group or session.
//! Without a cgroup to kill, this is the only way to stop a command's detached daemons.
//!
//! A process that double-forks normally ends up a child of init, so it no longer shows up as a
//! descendant. Making the root process a child subreaper keeps such processes in its subtree for as
//! long as the root lives.

use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;

/// Makes the calling process a child subreaper (`PR_SET_CHILD_SUBREAPER`), so that its detached
/// descendants stay its descendants instead of being reparented to init. Async-signal-safe, for
/// use between fork and exec.
///
/// The process then adopts its orphaned descendants, and it does not reap them unless it waits for
/// all its children: they stay zombies until it exits. Waiting for one of them to exit by polling
/// its pid then waits until the subreaper exits, and a process that watches for its parent dying
/// through `getppid() == 1` does not see it.
pub fn set_child_subreaper() -> io::Result<()> {
    // SAFETY: `prctl` with `PR_SET_CHILD_SUBREAPER` takes no pointers.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// A process, identified by its pid and start time so that a reused pid is never mistaken for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessId {
    pid: i32,
    start_time: u64,
}

impl ProcessId {
    pub fn pid(&self) -> i32 {
        self.pid
    }

    fn is_running(&self) -> bool {
        read_stat(self.pid).is_some_and(|stat| stat.start_time == self.start_time)
    }

    /// Sends `SIGKILL` to the process if it is still running and is still the same process. A
    /// process that is gone is not an error.
    ///
    /// Signals it through a pidfd, which refers to one process for good: once the start time
    /// checked after opening it matches, the signal cannot reach a process that reused the pid.
    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: `pidfd_open` takes no pointers.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, self.pid, 0) };
        if fd < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::ESRCH) => Ok(()),
                // Kernels before 5.3: a pid reused between the check and the signal gets it.
                Some(libc::ENOSYS) if self.is_running() => {
                    // SAFETY: `kill` has no memory safety requirements.
                    ignore_esrch(unsafe { libc::kill(self.pid, libc::SIGKILL) } as libc::c_long)
                }
                Some(libc::ENOSYS) => Ok(()),
                _ => Err(e),
            };
        }
        // SAFETY: `pidfd_open` returned a new file descriptor that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(fd as libc::c_int) };
        if !self.is_running() {
            return Ok(());
        }
        // SAFETY: `fd` is a pidfd; a null `siginfo` is allowed and sends a plain `SIGKILL`.
        ignore_esrch(unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        })
    }
}

fn ignore_esrch(res: libc::c_long) -> io::Result<()> {
    let e = io::Error::last_os_error();
    if res == 0 || e.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(e)
    }
}

struct Stat {
    state: char,
    ppid: i32,
    start_time: u64,
}

/// Fields of `/proc/<pid>/stat`. The command name may contain anything, so the fields are read
/// after its closing `)`.
fn parse_stat(stat: &str) -> Option<Stat> {
    let (_, rest) = stat.rsplit_once(')')?;
    // Fields after the command name start at field 3 (`state`); `ppid` is field 4 and
    // `starttime` is field 22.
    let mut fields = rest.split_whitespace();
    let state = fields.next()?.chars().next()?;
    let ppid = fields.next()?.parse().ok()?;
    let start_time = fields.nth(17)?.parse().ok()?;
    Some(Stat {
        state,
        ppid,
        start_time,
    })
}

fn read_stat(pid: i32) -> Option<Stat> {
    parse_stat(&fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

/// The state letter of `pid` in `/proc/<pid>/stat`, such as `Z` for a zombie.
pub(crate) fn process_state(pid: i32) -> Option<char> {
    Some(read_stat(pid)?.state)
}

/// All live descendants of `root`, not including `root` itself. Reads all of `/proc`.
pub fn descendants(root: i32) -> Vec<ProcessId> {
    walk(&mut children_by_parent(), vec![root])
}

/// All live descendants of `leader` and of those of `roots` that are still running, not including
/// them. `leader` must be a child of this process that it has not reaped, so that its pid is still
/// its own. Reads all of `/proc`.
pub fn descendants_of(leader: Option<i32>, roots: &[ProcessId]) -> Vec<ProcessId> {
    let roots = leader
        .into_iter()
        .chain(roots.iter().filter(|r| r.is_running()).map(|r| r.pid))
        .collect();
    walk(&mut children_by_parent(), roots)
}

fn children_by_parent() -> BTreeMap<i32, Vec<ProcessId>> {
    let mut children: BTreeMap<i32, Vec<ProcessId>> = BTreeMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return children;
    };
    for pid in entries.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok()) {
        if let Some(stat) = read_stat(pid) {
            children.entry(stat.ppid).or_default().push(ProcessId {
                pid,
                start_time: stat.start_time,
            });
        }
    }
    children
}

fn walk(children: &mut BTreeMap<i32, Vec<ProcessId>>, roots: Vec<i32>) -> Vec<ProcessId> {
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
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    use super::*;

    #[test]
    fn test_parse_stat() {
        let stat =
            "1234 (a) b (c)) S 42 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 98765 0 0";
        let stat = parse_stat(stat).unwrap();
        assert_eq!((stat.state, stat.ppid, stat.start_time), ('S', 42, 98765));
        assert!(parse_stat("garbage").is_none());
    }

    #[test]
    fn test_descendants_of_self_include_a_child_and_kill_it() -> io::Result<()> {
        // ast-grep-ignore: rust/buck2-no-command-new
        let mut child = Command::new("sleep").arg("1000").spawn()?;
        let found: Vec<ProcessId> = descendants(std::process::id() as i32)
            .into_iter()
            .filter(|p| p.pid() == child.id() as i32)
            .collect();
        if found.is_empty() {
            child.kill()?;
            child.wait()?;
            panic!("a direct child should be a descendant");
        }
        found[0].kill()?;
        assert_eq!(child.wait()?.signal(), Some(libc::SIGKILL));
        Ok(())
    }

    #[test]
    fn test_kill_skips_a_process_that_is_no_longer_the_same() -> io::Result<()> {
        // ast-grep-ignore: rust/buck2-no-command-new
        let mut child = Command::new("sleep").arg("1000").spawn()?;
        let stale = ProcessId {
            pid: child.id() as i32,
            start_time: 0,
        };
        stale.kill()?;
        assert_eq!(
            crate::child_state(child.id() as i32),
            crate::ChildState::Running
        );
        child.kill()?;
        child.wait()?;
        Ok(())
    }
}
