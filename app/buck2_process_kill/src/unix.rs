/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::io;
use std::mem::MaybeUninit;

/// What has become of a child of this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    Running,
    /// Exited and not reaped: a zombie, whose pid, and process group id if it led one, cannot be
    /// reused until its owner reaps it.
    Exited,
    /// Reaped already: its pid may name another process, or another process group, by now.
    Reaped,
}

/// The state of `pid`, a child of this process. Never reaps it.
pub fn child_state(pid: i32) -> ChildState {
    loop {
        match waitid_exited(pid, libc::WNOHANG) {
            // With `WNOHANG`, `si_pid` stays 0 while the child is still running.
            // SAFETY: `waitid` fills `siginfo_t` as for `SIGCHLD`, which has `si_pid`.
            Ok(info) if unsafe { info.si_pid() } == 0 => return ChildState::Running,
            Ok(info) if is_exit(&info) => return ChildState::Exited,
            // A stop, which macOS reports even without `WSTOPPED`.
            Ok(_) => return ChildState::Running,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.raw_os_error() == Some(libc::ECHILD) => return ChildState::Reaped,
            // Unexpected: report it as running, so that a caller's grace period is not cut short.
            Err(_) => return ChildState::Running,
        }
    }
}

/// Blocks until `pid`, a child of this process, exits. Never reaps it, like [`child_state`].
pub fn wait_for_child_exit(pid: i32) -> io::Result<()> {
    loop {
        match waitid_exited(pid, 0) {
            Ok(info) if is_exit(&info) => return Ok(()),
            // A stop, which macOS reports even without `WSTOPPED`, and goes on reporting.
            Ok(_) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

fn is_exit(info: &libc::siginfo_t) -> bool {
    matches!(
        info.si_code,
        libc::CLD_EXITED | libc::CLD_KILLED | libc::CLD_DUMPED
    )
}

fn waitid_exited(pid: i32, flags: libc::c_int) -> io::Result<libc::siginfo_t> {
    let mut info = MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: `info` is a valid, aligned and exclusively owned `siginfo_t` for `waitid` to write,
    // and the flags are valid for `waitid`. `WNOWAIT` leaves the child unreaped.
    let res = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOWAIT | flags,
        )
    };
    if res == 0 {
        // SAFETY: zero-initialized, then filled in by a successful `waitid`.
        Ok(unsafe { info.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Whether `pid`, a process that is not a child of this one, is gone. A zombie counts as gone:
/// only its own parent can reap it, and it runs no more code.
pub fn process_is_gone(pid: i32) -> io::Result<bool> {
    // SAFETY: signal 0 only checks that `pid` exists and may be signalled.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(is_zombie(pid));
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::ESRCH) {
        Ok(true)
    } else {
        Err(e)
    }
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: i32) -> bool {
    crate::linux::process_state(pid) == Some('Z')
}

#[cfg(not(target_os = "linux"))]
fn is_zombie(_pid: i32) -> bool {
    false
}

/// Sends `SIGTERM` to `pid` alone. A process that is already gone is not an error.
pub fn terminate(pid: i32) -> io::Result<()> {
    // SAFETY: `kill` has no memory safety requirements.
    ignore_esrch(unsafe { libc::kill(pid, libc::SIGTERM) })
}

/// Sends `SIGKILL` to every process in the group `pgid`. An empty group is not an error.
pub fn kill_group(pgid: i32) -> io::Result<()> {
    // SAFETY: `killpg` has no memory safety requirements.
    ignore_esrch(unsafe { libc::killpg(pgid, libc::SIGKILL) })
}

fn ignore_esrch(res: libc::c_int) -> io::Result<()> {
    let e = io::Error::last_os_error();
    if res == 0 || e.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(e)
    }
}

/// Kills the process group `pgid` when dropped, unless disarmed first. For a kill that may be
/// abandoned before its final `SIGKILL`, for example when its future is dropped during a grace
/// period.
///
/// The group's leader must still be unreaped when the guard is dropped, or its pid, and so the
/// group id, may name another group by then.
pub struct KillGroupOnDrop {
    pgid: Option<i32>,
}

impl KillGroupOnDrop {
    pub fn new(pgid: i32) -> Self {
        Self { pgid: Some(pgid) }
    }

    /// For a caller about to kill the group itself.
    pub fn disarm(&mut self) {
        self.pgid = None;
    }
}

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            let _ignored = kill_group(pgid);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Child;
    use std::process::Command;
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    fn spawn_sh(script: &str, new_group: bool) -> Child {
        // ast-grep-ignore: rust/buck2-no-command-new
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(script);
        if new_group {
            cmd.process_group(0);
        }
        cmd.spawn().expect("should spawn sh")
    }

    fn eventually(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn test_an_exited_child_is_reported_and_left_unreaped() {
        let mut child = spawn_sh("exit 7", false);
        let pid = child.id() as i32;
        assert!(eventually(Duration::from_secs(30), || {
            child_state(pid) == ChildState::Exited
        }));
        let status = child
            .wait()
            .expect("the child should still be there to reap");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn test_a_running_child_has_not_exited() {
        let mut child = spawn_sh("sleep 1000", false);
        assert_eq!(child_state(child.id() as i32), ChildState::Running);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn test_a_reaped_child_is_reported_as_such() {
        let mut child = spawn_sh("exit 0", false);
        child.wait().unwrap();
        assert_eq!(child_state(child.id() as i32), ChildState::Reaped);
    }

    #[test]
    fn test_a_stopped_child_is_running() {
        let mut child = spawn_sh("sleep 1000", false);
        let pid = child.id() as i32;
        // SAFETY: `kill` has no memory safety requirements.
        unsafe { libc::kill(pid, libc::SIGSTOP) };
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(child_state(pid), ChildState::Running);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn test_wait_for_child_exit_leaves_it_unreaped() {
        let mut child = spawn_sh("exit 3", false);
        wait_for_child_exit(child.id() as i32).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(3));
    }

    #[test]
    fn test_terminate_and_kill_group() {
        use std::io::BufRead;
        use std::io::BufReader;
        use std::process::Stdio;

        // ast-grep-ignore: rust/buck2-no-command-new
        let mut leader = Command::new("sh")
            .arg("-c")
            .arg("trap 'exit 5' TERM; sleep 1000 & echo ready; wait")
            .process_group(0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(leader.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pgid = leader.id() as i32;
        terminate(pgid).unwrap();
        assert_eq!(leader.wait().unwrap().code(), Some(5));
        // The group outlives its leader while the leader's `sleep` is in it.
        kill_group(pgid).unwrap();
    }

    #[test]
    fn test_dropping_the_guard_kills_the_group_unless_disarmed() {
        let mut child = spawn_sh("sleep 1000", true);
        drop(KillGroupOnDrop::new(child.id() as i32));
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGKILL));

        let mut child = spawn_sh("sleep 1000", true);
        let mut guard = KillGroupOnDrop::new(child.id() as i32);
        guard.disarm();
        drop(guard);
        assert_eq!(child_state(child.id() as i32), ChildState::Running);
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_process_is_gone_counts_a_zombie_as_gone() {
        use std::io::BufRead;
        use std::io::BufReader;
        use std::process::Stdio;

        // `sleep 0` exits at once, and its parent, which `exec`s `sleep 1000`, never reaps it.
        // ast-grep-ignore: rust/buck2-no-command-new
        let mut parent = Command::new("sh")
            .arg("-c")
            .arg("sleep 0 & echo $!; exec sleep 1000")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(parent.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let zombie: i32 = line.trim().parse().unwrap();

        assert!(eventually(Duration::from_secs(30), || {
            crate::linux::process_state(zombie) == Some('Z')
        }));
        assert!(process_is_gone(zombie).unwrap());
        assert!(!process_is_gone(parent.id() as i32).unwrap());

        parent.kill().unwrap();
        parent.wait().unwrap();
    }
}
