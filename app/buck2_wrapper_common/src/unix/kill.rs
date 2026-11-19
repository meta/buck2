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
        Some(sysinfo::ProcessStatus::Zombie) => false,
        Some(_) => true,
        None => false,
    })
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
}
