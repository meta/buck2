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

/// Send `KILL` or call `TerminateProcess` on the given process.
///
/// Returns a KilledProcessHandle that can be used to observe the termination of the killed process.
pub fn kill(pid: Pid) -> buck2_error::Result<Option<KilledProcessHandle>> {
    match imp::kill(pid)? {
        Some(handle) => Ok(Some(KilledProcessHandle { handle })),
        None => Ok(None),
    }
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
