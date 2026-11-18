/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Cross-platform process killing.

use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use sysinfo::Process;
use sysinfo::ProcessRefreshKind;
use sysinfo::ProcessesToUpdate;
use sysinfo::System;

use crate::pid::Pid;
#[cfg(unix)]
use crate::unix::kill as imp;
#[cfg(windows)]
use crate::win::kill as imp;

pub fn process_creation_time(process: &Process) -> Option<Duration> {
    imp::process_creation_time(process)
}

pub fn process_exists(pid: Pid) -> buck2_error::Result<bool> {
    imp::process_exists(pid)
}

/// Start time of the given process in seconds since the unix epoch, with 1
/// second precision, or `None` if the process does not exist or its start
/// time cannot be read.
///
/// A `(pid, start time)` pair identifies a process instance where the pid
/// alone is ambiguous (pid reuse, or a pid recorded inside a pid namespace).
/// Writers and readers of recorded start times must both use this function so
/// the values are comparable. The value derives from the boot time, which moves
/// when the wall clock is stepped, so a reader that concludes from a mismatch
/// that the process exited should allow for that, as
/// [`recorded_process_may_be_running`] does.
///
/// `None` names the current process, for recording its own start time.
pub fn process_start_time(pid: Option<Pid>) -> Option<u64> {
    let pid = match pid {
        Some(pid) => sysinfo::Pid::from_u32(pid.to_u32()),
        None => sysinfo::Pid::from_u32(std::process::id()),
    };
    let mut system = System::new().expect("should be supported on this platform");
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    let proc = system.process(pid)?;
    // sysinfo reports 0, not an error, when the start time is unavailable.
    Some(proc.start_time()).filter(|t| *t != 0)
}

/// How far a process's start time may stray from the recorded one and still name the recorded
/// process. Start times are derived from the boot time, which moves when the wall clock is
/// stepped, and have one-second precision; a process that reused the pid started long after.
const RECORDED_START_TIME_TOLERANCE: u64 = 2;

/// How much later than its record a process may appear to have started and still be taken for
/// the recorded one, when the record has no start time. Covers the one-second precision of both
/// times and modest wall-clock adjustments.
const UNRECORDED_START_TIME_TOLERANCE: Duration = Duration::from_secs(60);

/// Whether the process recorded as `pid` may still be running: `false` only when no process has
/// that pid, or the process that has it is provably another one.
///
/// `start_time` is the recorded [`process_start_time`], `0` if the recorder could not read it;
/// a recorded start time identifies the process, to within a couple of seconds of clock
/// adjustment. Without one, a process that started well after `recorded_at`, when the record was
/// written, reused the pid. A process whose start time cannot be read is taken to be the recorded
/// one.
pub fn recorded_process_may_be_running(
    pid: Pid,
    start_time: u64,
    recorded_at: Option<SystemTime>,
) -> buck2_error::Result<bool> {
    if !process_exists(pid)? {
        return Ok(false);
    }
    let Some(actual) = process_start_time(Some(pid)) else {
        return Ok(true);
    };
    if start_time != 0 {
        return Ok(actual.abs_diff(start_time) <= RECORDED_START_TIME_TOLERANCE);
    }
    Ok(recorded_at
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .is_none_or(|at| actual <= (at + UNRECORDED_START_TIME_TOLERANCE).as_secs()))
}

/// The `argv[0]` a tenant proxy is launched with, and the flag that names its spec file; the
/// process-tree kill recognizes proxies by them.
pub const TENANT_PROXY_ARG0: &str = "buck2d-tenant-proxy";
pub const TENANT_PROXY_FILE_FLAG: &str = "tenant-proxy-file";

/// Send `KILL` or call `TerminateProcess` on the given process.
///
/// Returns a KilledProcessHandle that can be used to observe the termination of the killed process.
pub fn kill(pid: Pid) -> buck2_error::Result<Option<KilledProcessHandle>> {
    match imp::kill(pid)? {
        Some(handle) => Ok(Some(KilledProcessHandle { handle })),
        None => Ok(None),
    }
}

/// Emergency shutdown for a shared daemon on Linux. Tenant proxies among the descendants are
/// left alone: each exits on its own once it sees the daemon gone, after clearing its slot.
///
/// Stops the tree, so that nothing forks away while it is walked, then kills it, in passes of at
/// most 400 milliseconds until the tree is confirmed dead or `limit` has passed. Everything a pass
/// stops stays stopped until it is killed, so no part of the tree runs unwatched between passes.
/// A descendant in uninterruptible sleep cannot stop; it is killed instead, which it cannot
/// escape, and counts as dead although it only exits once it wakes. Returns `Ok(false)` if the
/// tree is not confirmed dead within `limit`: everything that was signalled is then killed
/// rather than left stopped.
#[cfg(target_os = "linux")]
pub fn kill_unidaemon_tree(
    pid: Pid,
    start_time: u64,
    limit: Duration,
) -> buck2_error::Result<bool> {
    use std::time::Instant;

    let give_up = Instant::now() + limit;
    let mut signalled = Vec::new();
    let result = loop {
        let pass = give_up
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(400));
        // A pass can fail on a process that changes under it (it exits mid-read, or turns out to
        // be unsignallable); a later pass sees a different tree, so retry until `limit`.
        match stop_and_kill_tree(pid, start_time, pass, &mut signalled) {
            Ok(false) if Instant::now() < give_up => std::thread::sleep(Duration::from_millis(2)),
            // Errors fail a pass at once, so back off rather than rescan `/proc` in a tight loop.
            Err(_) if Instant::now() < give_up => std::thread::sleep(Duration::from_millis(50)),
            result => break result,
        }
    };
    // Also after success: a pass that finds the root gone returns at once, and must not leave
    // stopped what an earlier pass stopped. Only processes that are still the ones signalled: an
    // identifier freed meanwhile may belong to an unrelated process by now.
    for (pid, start) in signalled.into_iter().rev() {
        if same_process(pid, start) {
            let _ignored = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid as i32),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    result
}

/// The state letter and start time of `pid`, from `/proc/<pid>/stat`.
#[cfg(target_os = "linux")]
fn state(pid: u32) -> Option<(u8, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let status = fields.next()?.as_bytes()[0];
    let start = fields.nth(18)?.parse().ok()?;
    Some((status, start))
}

/// Whether `pid` still names the process whose `/proc` start time was `start`.
#[cfg(target_os = "linux")]
fn same_process(pid: u32, start: u64) -> bool {
    state(pid).is_some_and(|(_, current)| current == start)
}

/// [`kill_unidaemon_tree`] for callers that only need to know whether the tree was confirmed
/// dead within `limit`.
#[cfg(target_os = "linux")]
pub fn kill_unidaemon_tree_within(pid: Pid, start_time: u64, limit: Duration) -> bool {
    matches!(kill_unidaemon_tree(pid, start_time, limit), Ok(true))
}

#[cfg(target_os = "linux")]
fn stop_and_kill_tree(
    pid: Pid,
    start_time: u64,
    timeout: Duration,
    signalled: &mut Vec<(u32, u64)>,
) -> buck2_error::Result<bool> {
    use std::time::Instant;

    use buck2_hash::BuckMutMap;
    use nix::sys::signal::Signal;
    use nix::sys::signal::kill as signal;
    use nix::sys::signal::killpg;

    fn send(pid: u32, sig: Signal) -> buck2_error::Result<()> {
        signal(nix::unistd::Pid::from_raw(pid as i32), sig).or_else(|error| {
            if error == nix::errno::Errno::ESRCH {
                Ok(())
            } else {
                Err(error)
            }
        })?;
        Ok(())
    }
    fn process_parents() -> buck2_error::Result<BuckMutMap<u32, u32>> {
        let mut parents = BuckMutMap::default();
        // The task/children proc file is absent on kernels without CONFIG_PROC_CHILDREN.
        // A shallow TGID snapshot also avoids scanning every thread with sysinfo.
        for entry in std::fs::read_dir("/proc")? {
            // Processes exit throughout the scan; a process that cannot be read is not one we
            // could have stopped either.
            let Ok(entry) = entry else {
                continue;
            };
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            // A process that exits between opening and reading its stat fails with `ESRCH`,
            // which is not `NotFound`.
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            if let Some(parent) = stat
                .rsplit_once(')')
                .and_then(|(_, fields)| fields.split_whitespace().nth(1))
                .and_then(|parent| parent.parse().ok())
            {
                parents.insert(pid, parent);
            }
        }
        Ok(parents)
    }
    let caller = std::process::id();
    let root = pid.to_u32();
    // Not `recorded_process_may_be_running`: its existence check has sysinfo read every thread
    // under `/proc`, which takes longer than a whole pass on a busy host.
    if start_time != 0
        && process_start_time(Some(pid))
            .is_none_or(|actual| actual.abs_diff(start_time) > RECORDED_START_TIME_TOLERANCE)
    {
        return Ok(true);
    }
    let deadline = Instant::now() + timeout;
    let mut root_start = None;
    if root != caller {
        let Some((_, start)) = state(root) else {
            return Ok(true);
        };
        root_start = Some(start);
        signalled.push((root, start));
        send(root, Signal::SIGSTOP)?;
        while let Some((status, _)) = state(root)
            && status != b'T'
            && status != b't'
            && status != b'Z'
        {
            if Instant::now() >= deadline {
                // In uninterruptible sleep the root cannot fork either, so walk on.
                if status == b'D' {
                    break;
                }
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    let mut descendants = BuckMutMap::default();
    loop {
        let before = descendants.len();
        let parents = process_parents()?;
        loop {
            let count = descendants.len();
            for (&child, &parent) in &parents {
                if child == caller
                    || descendants.contains_key(&child)
                    || (parent != root && !descendants.contains_key(&parent))
                {
                    continue;
                }
                let command = std::fs::read(format!("/proc/{child}/cmdline")).unwrap_or_default();
                if command.split(|byte| *byte == 0).any(|arg| {
                    arg == TENANT_PROXY_ARG0.as_bytes()
                        || arg.strip_prefix(b"--") == Some(TENANT_PROXY_FILE_FLAG.as_bytes())
                }) {
                    continue;
                }
                let Some((status, start)) = state(child) else {
                    continue;
                };
                if status == b'Z' {
                    continue;
                }
                signalled.push((child, start));
                send(child, Signal::SIGSTOP)?;
                // A child in uninterruptible sleep cannot stop until it wakes, so it is killed
                // instead: with a kill pending it cannot fork, and it stays in place while the walk
                // reaches its own children. Any other child gets until the deadline to stop.
                let stop_by = (Instant::now() + Duration::from_millis(50)).min(deadline);
                while let Some((status, _)) = state(child)
                    && status != b'T'
                    && status != b't'
                    && status != b'Z'
                {
                    if status == b'D' && Instant::now() >= stop_by {
                        if same_process(child, start) {
                            send(child, Signal::SIGKILL)?;
                        }
                        break;
                    }
                    if Instant::now() >= deadline {
                        return Ok(false);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                descendants.insert(child, start);
            }
            if count == descendants.len() {
                break;
            }
        }
        if before == descendants.len() {
            break;
        }
    }
    // Every kill checks that the identifier still names the process that was stopped: one freed
    // meanwhile may belong to an unrelated process by now.
    for (child, start) in &descendants {
        if !same_process(*child, *start) {
            continue;
        }
        let process = nix::unistd::Pid::from_raw(*child as i32);
        if nix::unistd::getpgid(Some(process)).ok() == Some(process) {
            let _ignored = killpg(process, Signal::SIGKILL);
        }
        send(*child, Signal::SIGKILL)?;
    }
    // Every descendant has a kill pending. One still in uninterruptible sleep dies when it leaves
    // it and cannot run until then, so it does not hold back the root.
    while descendants.iter().any(|(child, start)| {
        state(*child)
            .is_some_and(|(status, current)| status != b'Z' && status != b'D' && current == *start)
    }) {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if let Some(root_start) = root_start
        && same_process(root, root_start)
    {
        send(root, Signal::SIGKILL)?;
        while state(root).is_some_and(|(status, _)| status != b'Z') {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(true)
}

pub struct KilledProcessHandle {
    handle: imp::KilledProcessHandleImpl,
}

impl KilledProcessHandle {
    pub fn has_exited(&self) -> buck2_error::Result<bool> {
        self.handle.has_exited()
    }
}

/// Get the status of a given process according to sysinfo.
pub fn get_sysinfo_status(pid: Pid) -> Option<sysinfo::ProcessStatus> {
    use sysinfo::ProcessRefreshKind;
    use sysinfo::System;

    let pid = sysinfo::Pid::from_u32(pid.to_u32());

    let mut system = System::new().expect("should be supported on this platform");
    // There is some bug in `sysinfo` so we have to use `refresh_processes_specifics`
    // instead of `refresh_process_specifics`, otherwise we not always get process info.
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());

    let proc = system.process(pid)?;
    Some(proc.status())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;
    use std::time::SystemTime;

    use crate::background_command;
    use crate::kill::UNRECORDED_START_TIME_TOLERANCE;
    use crate::kill::kill;
    use crate::kill::process_exists;
    use crate::kill::process_start_time;
    use crate::kill::recorded_process_may_be_running;
    use crate::pid::Pid;

    #[test]
    fn test_recorded_process_may_be_running() {
        let pid = Pid::from_u32(std::process::id()).unwrap();
        let start_time = process_start_time(None).expect("own start time should be readable");
        assert!(
            recorded_process_may_be_running(pid, start_time, None).unwrap(),
            "the recorded start time identifies the running process"
        );
        assert!(
            recorded_process_may_be_running(pid, start_time - 1, None).unwrap(),
            "a start time a second off, as after a clock step, names the same process"
        );
        assert!(
            !recorded_process_may_be_running(pid, start_time - 60, None).unwrap(),
            "a start time a minute off means the pid was reused"
        );
        assert!(
            recorded_process_may_be_running(pid, 0, Some(SystemTime::now())).unwrap(),
            "without a start time, a process that started before the record may be the recorded one"
        );
        let long_before_start = SystemTime::UNIX_EPOCH + Duration::from_secs(start_time)
            - UNRECORDED_START_TIME_TOLERANCE
            - Duration::from_secs(2);
        assert!(
            !recorded_process_may_be_running(pid, 0, Some(long_before_start)).unwrap(),
            "a process that started well after the record reused the pid"
        );
        assert!(
            recorded_process_may_be_running(pid, 0, None).unwrap(),
            "with neither a start time nor a record time, a live pid may be the recorded one"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_unidaemon_fallback_verifies_descendants_and_identity() -> buck2_error::Result<()> {
        let directory = tempfile::tempdir()?;
        let marker = directory.path().join("child");
        let mut command = background_command("sh");
        command
            .args(["-c", "sleep 1000 & echo $! > \"$1\"; wait", "--"])
            .arg(&marker);
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut parent = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(&marker)
            .ok()
            .is_none_or(|contents| contents.trim().is_empty())
        {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let pid = Pid::from_u32(parent.id())?;
        let child = Pid::from_u32(std::fs::read_to_string(marker)?.trim().parse()?)?;
        let start = super::process_start_time(Some(pid)).expect("child start time available");
        // A recorded start time beyond the clock-step tolerance names another process.
        assert!(super::kill_unidaemon_tree(
            pid,
            start + super::RECORDED_START_TIME_TOLERANCE + 1,
            Duration::from_millis(400)
        )?);
        assert!(process_exists(pid)?);
        assert!(super::kill_unidaemon_tree(
            pid,
            start,
            Duration::from_millis(400)
        )?);
        parent.wait()?;
        assert!(!process_exists(child)?);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_unidaemon_fallback_out_of_time_leaves_nothing_stopped() -> buck2_error::Result<()> {
        let mut command = background_command("sleep");
        command
            .arg("1000")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut process = command.spawn()?;
        let pid = Pid::from_u32(process.id())?;
        let start = super::process_start_time(Some(pid)).expect("process start time available");
        // With no time at all the call may give up right after stopping the process, which
        // must then still be killed rather than left stopped.
        let _confirmed = super::kill_unidaemon_tree(pid, start, Duration::ZERO)?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = process.try_wait()? {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the process was left stopped instead of killed"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(!status.success(), "the process should have been killed");
        Ok(())
    }

    #[test]
    fn test_process_exists_kill() {
        let mut command = if !cfg!(windows) {
            let mut command = background_command("sh");
            command.args(["-c", "sleep 10000"]);
            command
        } else {
            let mut command = background_command("powershell");
            command.args(["-c", "Start-Sleep -Seconds 10000"]);
            command
        };
        let mut child = command.spawn().unwrap();
        let pid = Pid::from_u32(child.id()).unwrap();
        // TODO T187306095: we only check for existence once, because flakiness
        assert!(
            process_exists(pid).unwrap(),
            "process should exist; attempt 1; pid {pid}"
        );

        let handle = kill(pid).unwrap().unwrap();

        child.wait().unwrap();
        // Drop child to ensure the Windows handle is closed.
        drop(child);

        if !cfg!(windows) {
            assert!(handle.has_exited().unwrap());
        } else {
            let start = Instant::now();
            loop {
                if handle.has_exited().unwrap() {
                    break;
                }
                assert!(
                    Instant::now() - start < Duration::from_secs(20),
                    "Timed out waiting for process to die"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
