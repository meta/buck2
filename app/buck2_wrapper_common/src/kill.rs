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
/// the values compare exactly.
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

    use crate::background_command;
    use crate::kill::kill;
    use crate::kill::process_exists;
    use crate::pid::Pid;

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
