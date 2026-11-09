/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::os::unix::process::CommandExt;
use std::process::Command as StdCommand;
use std::process::ExitStatus;
use std::time::Duration;
use std::time::Instant;

use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_resource_control::ActionFreezeEvent;
use buck2_resource_control::ActionFreezeEventReceiver;
use buck2_resource_control::OrphanProcessInfo;
use buck2_resource_control::cgroup::Cgroup;
use buck2_resource_control::cgroup::CgroupKindLeaf;
use buck2_resource_control::cgroup::CgroupMinimal;
use buck2_resource_control::cgroup::NoMemoryMonitoring;
use buck2_resource_control::path::CgroupPathBuf;
use futures::StreamExt;
use futures::pin_mut;
use nix::errno::Errno;
use nix::sys::signal;
use nix::sys::signal::Signal;
use nix::unistd::Pid;
use rustix::process::WaitId;
use rustix::process::WaitIdOptions;
use tokio::pin;
use tokio::process::Child;
use tokio::process::ChildStderr;
use tokio::process::ChildStdout;
use tokio::process::Command;

use crate::process_group::SpawnError;

pub(crate) struct ProcessCommandImpl {
    inner: Command,
    cgroup: Option<Cgroup<NoMemoryMonitoring, CgroupKindLeaf>>,
}

impl ProcessCommandImpl {
    pub(crate) async fn new(
        mut cmd: StdCommand,
        cgroup: Option<CgroupPathBuf>,
    ) -> buck2_error::Result<Self> {
        cmd.process_group(0);

        let cgroup = if let Some(cgroup) = cgroup {
            let cgroup = CgroupMinimal::try_from_path(cgroup.clone())
                .await?
                .into_leaf()
                .await?;
            cgroup.setup_command(&mut cmd);
            Some(cgroup)
        } else {
            None
        };

        Ok(Self {
            inner: cmd.into(),
            cgroup,
        })
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn spawn(mut self) -> Result<ProcessGroupImpl, (SpawnError, Self)> {
        match self.inner.spawn() {
            Ok(inner) => Ok(ProcessGroupImpl {
                inner,
                cgroup: self.cgroup,
            }),
            Err(e) => Err((e.into(), self)),
        }
    }
}

pub(crate) struct ProcessGroupImpl {
    inner: Child,
    cgroup: Option<Cgroup<NoMemoryMonitoring, CgroupKindLeaf>>,
}

impl ProcessGroupImpl {
    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.inner.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.inner.stderr.take()
    }

    /// It might seem surprising that we associate the freezing/unfreezing with the lifetime of a
    /// `wait` future instead of with the lifetime of some higher level thing such as the process,
    /// the command, the action, etc.
    ///
    /// But this is intentional and important; it ensures that we only freeze cgroups while we know
    /// exactly what's going on in them. Any other choice risks leaving behind frozen cgroups or
    /// freezing cgroups during critical times like when we're spawning into them or killing them.
    pub(crate) async fn wait(
        &mut self,
        freeze_rx: impl ActionFreezeEventReceiver,
    ) -> buck2_error::Result<(ExitStatus, Vec<OrphanProcessInfo>)> {
        let child = self.inner.wait();
        pin!(child);
        pin_mut!(freeze_rx);
        let mut freeze_guard = None;
        loop {
            tokio::select! {
                res = &mut child => {
                    // Kill any remaining PIDs in the cgroup that outlived the main process.
                    let orphans = if let Some(cgroup) = self.cgroup.as_ref() {
                        cgroup.kill_remaining_pids().await?
                    } else {
                        Vec::new()
                    };
                    break Ok((res?, orphans));
                },
                Some(freeze_op) = freeze_rx.next() => {
                    match freeze_op {
                        ActionFreezeEvent::Unfreeze => {
                            drop(freeze_guard.take());
                        }
                        ActionFreezeEvent::Freeze => {
                            if let Some(cgroup) = self.cgroup.as_ref() {
                                freeze_guard = cgroup.freeze().await.ok();
                            }
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    /// Kills the process group in three steps:
    /// 1. With a grace period, `SIGTERM` the leader alone. It can then report what it was doing,
    ///    and query the processes it started, which keep running.
    /// 2. Wait up to the grace period for the leader to exit, without reaping it. An exited but
    ///    unreaped leader is a zombie whose pid cannot be reused, so the pid, which is also the
    ///    group id, still names this group in step 3. Reaping it would free the pid for any new
    ///    process, such as the leader of another action's group (buck2 starts every action in its
    ///    own group), and step 3 would then kill that unrelated group.
    /// 3. `SIGKILL` the whole group, whether or not the leader exited, so that nothing it started
    ///    outlives it.
    ///
    /// This holds for every graceful kill: a timed-out test's `timeout_grace_period_s` and a
    /// worker's `graceful_shutdown_timeout_s` alike. If the kill is abandoned before step 3, the
    /// group still gets `SIGKILL` (see `KillGroupOnDrop`).
    pub(crate) async fn kill(
        &mut self,
        graceful_shutdown_timeout_s: Option<u32>,
    ) -> buck2_error::Result<()> {
        let pid: i32 = self
            .inner
            .id()
            .and_then(|id| id.try_into().ok())
            .internal_error("PID does not fit a i32")?;
        let pgid = Pid::from_raw(pid);
        let mut kill_on_drop = KillGroupOnDrop(Some(pgid));

        if let Some(graceful_shutdown_timeout_s) = graceful_shutdown_timeout_s {
            terminate_leader(pgid)?;
            wait_for_exit_without_reaping(
                pgid,
                Duration::from_secs(graceful_shutdown_timeout_s.into()),
            )
            .await;
        }

        kill_on_drop.0 = None;
        kill_group(pgid)
    }
}

/// Sends `SIGTERM` to the leader of the process group `pgid` alone. A leader that is already gone
/// is not an error: the rest of its group still has to be killed.
fn terminate_leader(pgid: Pid) -> buck2_error::Result<()> {
    match signal::kill(pgid, Signal::SIGTERM) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(e) => Err(e).with_buck_error_context(|| format!("Failed to terminate process {pgid}")),
    }
}

/// Sends `SIGKILL` to every process in the group `pgid`. An empty group is not an error.
fn kill_group(pgid: Pid) -> buck2_error::Result<()> {
    match signal::killpg(pgid, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => Ok(()),
        Err(e) => Err(e).with_buck_error_context(|| format!("Failed to kill process group {pgid}")),
    }
}

/// Waits up to `timeout` for the child `pid` to exit and returns whether it did, without reaping
/// it.
///
/// The child is left a zombie so that its pid keeps naming its process group until the caller has
/// killed the group: once reaped, the pid can be reused, including as the id of a new process group
/// (buck2 starts every action in its own group), which a later `killpg` would then hit. This is why
/// it polls `waitid(WNOWAIT)` instead of using `tokio::process::Child::wait`, which reaps.
async fn wait_for_exit_without_reaping(pid: Pid, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if has_exited(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Whether the child `pid` has exited, leaving it unreaped.
fn has_exited(pid: Pid) -> bool {
    let Some(pid) = rustix::process::Pid::from_raw(pid.as_raw()) else {
        return true;
    };
    loop {
        match rustix::process::waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
        ) {
            // With `NOHANG`, there is no status while the child is still running.
            Ok(status) => return status.is_some(),
            Err(rustix::io::Errno::INTR) => {}
            // Nothing else reaps the leader: tokio only does so while its `wait` is polled, which
            // it is not during a kill. So no such child means it is gone.
            Err(rustix::io::Errno::CHILD) => return true,
            // Unexpected: keep the leader's grace period rather than cut it short.
            Err(_) => return false,
        }
    }
}

/// Kills the process group if a kill is abandoned before its final `SIGKILL`, for example when
/// the daemon shuts down during a grace period.
struct KillGroupOnDrop(Option<Pid>);

impl Drop for KillGroupOnDrop {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            let _ignored = signal::killpg(pgid, Signal::SIGKILL);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Child as StdChild;

    use buck2_util::process::background_command;

    use super::*;

    fn spawn_sh(script: &str) -> StdChild {
        background_command("sh")
            .arg("-c")
            .arg(script)
            .spawn()
            .expect("should spawn sh")
    }

    fn pid_of(child: &StdChild) -> Pid {
        Pid::from_raw(child.id().try_into().expect("pid should fit an i32"))
    }

    #[tokio::test]
    async fn test_wait_reports_an_exited_child_and_leaves_it_unreaped() {
        let mut child = spawn_sh("exit 7");
        assert!(
            wait_for_exit_without_reaping(pid_of(&child), Duration::from_secs(30)).await,
            "the child exits at once"
        );
        let status = child
            .wait()
            .expect("the child should still be there to reap");
        assert_eq!(status.code(), Some(7));
    }

    #[tokio::test]
    async fn test_wait_times_out_on_a_running_child() {
        let mut child = spawn_sh("sleep 1000");
        let start = Instant::now();
        let exited =
            wait_for_exit_without_reaping(pid_of(&child), Duration::from_millis(300)).await;
        let elapsed = Instant::now() - start;
        let _ignored = child.kill();
        let _ignored = child.wait();
        assert!(!exited, "the child is still running");
        assert!(
            elapsed >= Duration::from_millis(300),
            "the wait should last the whole timeout, elapsed: {elapsed:?}"
        );
    }

    #[test]
    fn test_an_already_reaped_child_counts_as_exited() {
        let mut child = spawn_sh("true");
        let pid = pid_of(&child);
        child.wait().expect("should reap the child");
        assert!(has_exited(pid), "a child that was reaped is gone");
    }
}
