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
use buck2_process_kill::ChildState;
use buck2_process_kill::KillGroupOnDrop;
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
use tokio::pin;
use tokio::process::Child;
use tokio::process::ChildStderr;
use tokio::process::ChildStdout;
use tokio::process::Command;

use crate::kill_on_drop::KillDetachedDescendantsOnDrop;
use crate::process_group::SpawnError;

pub(crate) struct ProcessCommandImpl {
    inner: Command,
    cgroup: Option<Cgroup<NoMemoryMonitoring, CgroupKindLeaf>>,
    kill_detached_descendants: bool,
}

impl ProcessCommandImpl {
    /// With `kill_detached_descendants`, killing the process group also kills the leader's
    /// descendants that left it, such as daemons. Linux only: the leader is spawned as a child
    /// subreaper for that, so that they stay its descendants.
    pub(crate) async fn new(
        mut cmd: StdCommand,
        cgroup: Option<CgroupPathBuf>,
        kill_detached_descendants: bool,
    ) -> buck2_error::Result<Self> {
        cmd.process_group(0);
        let kill_detached_descendants = cfg!(target_os = "linux") && kill_detached_descendants;
        #[cfg(target_os = "linux")]
        if kill_detached_descendants {
            // Best effort: a leader that cannot become a subreaper still runs, and its
            // double-forked descendants are not found. See `set_child_subreaper` for what being
            // one entails.
            // SAFETY: `set_child_subreaper` is async-signal-safe.
            unsafe {
                cmd.pre_exec(|| {
                    let _ignored = buck2_process_kill::set_child_subreaper();
                    Ok(())
                })
            };
        }

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
            kill_detached_descendants,
        })
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn spawn(mut self) -> Result<ProcessGroupImpl, (SpawnError, Self)> {
        match self.inner.spawn() {
            Ok(inner) => Ok(ProcessGroupImpl {
                inner,
                cgroup: self.cgroup,
                kill_detached_descendants: self.kill_detached_descendants,
            }),
            Err(e) => Err((e.into(), self)),
        }
    }
}

pub(crate) struct ProcessGroupImpl {
    inner: Child,
    cgroup: Option<Cgroup<NoMemoryMonitoring, CgroupKindLeaf>>,
    kill_detached_descendants: bool,
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
    /// group still gets `SIGKILL` (see `KillGroupOnDrop`). With `kill_detached_descendants`, the
    /// leader's descendants that left the group are killed too.
    pub(crate) async fn kill(
        &mut self,
        graceful_shutdown_timeout_s: Option<u32>,
    ) -> buck2_error::Result<()> {
        let pid: i32 = self
            .inner
            .id()
            .and_then(|id| id.try_into().ok())
            .internal_error("PID does not fit a i32")?;
        // Listed before the leader gets any signal: once it exits, its detached descendants are
        // reparented to init and can no longer be found. Killed when this function returns or its
        // future is dropped, after the group: `kill_on_drop`, declared later, is dropped first.
        let mut kill_detached = if self.kill_detached_descendants {
            KillDetachedDescendantsOnDrop::of(pid).await
        } else {
            KillDetachedDescendantsOnDrop::default()
        };
        let mut kill_on_drop = KillGroupOnDrop::new(pid);

        if let Some(graceful_shutdown_timeout_s) = graceful_shutdown_timeout_s {
            // A leader that is already gone is not an error: the rest of its group still has to
            // be killed.
            buck2_process_kill::terminate(pid)
                .with_buck_error_context(|| format!("Failed to terminate process {pid}"))?;
            wait_for_exit_without_reaping(
                pid,
                Duration::from_secs(graceful_shutdown_timeout_s.into()),
            )
            .await;
            kill_detached.refresh().await;
        }

        kill_on_drop.disarm();
        if buck2_process_kill::child_state(pid) == ChildState::Reaped {
            // Its pid, and so the group id, may be another group's by now.
            return Ok(());
        }
        buck2_process_kill::kill_group(pid)
            .with_buck_error_context(|| format!("Failed to kill process group {pid}"))
    }
}

/// Waits up to `timeout` for the child `pid` to exit, without reaping it, and returns its state:
/// `Running` if it did not exit in time.
///
/// The child is left a zombie so that its pid keeps naming its process group until the caller has
/// killed the group: once reaped, the pid can be reused, including as the id of a new process group
/// (buck2 starts every action in its own group), which a later `killpg` would then hit. This is why
/// it polls [`buck2_process_kill::child_state`] instead of using `tokio::process::Child::wait`,
/// which reaps. Nothing else reaps the leader meanwhile: tokio only does so while its `wait` is
/// polled, which it is not during a kill.
async fn wait_for_exit_without_reaping(pid: i32, timeout: Duration) -> ChildState {
    let deadline = Instant::now() + timeout;
    loop {
        let state = buck2_process_kill::child_state(pid);
        if state != ChildState::Running || Instant::now() >= deadline {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
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

    fn pid_of(child: &StdChild) -> i32 {
        child.id().try_into().expect("pid should fit an i32")
    }

    #[tokio::test]
    async fn test_wait_reports_an_exited_child_and_leaves_it_unreaped() {
        let mut child = spawn_sh("exit 7");
        assert_eq!(
            wait_for_exit_without_reaping(pid_of(&child), Duration::from_secs(30)).await,
            ChildState::Exited,
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
        let state = wait_for_exit_without_reaping(pid_of(&child), Duration::from_millis(300)).await;
        let elapsed = Instant::now() - start;
        let _ignored = child.kill();
        let _ignored = child.wait();
        assert_eq!(state, ChildState::Running, "the child is still running");
        assert!(
            elapsed >= Duration::from_millis(300),
            "the wait should last the whole timeout, elapsed: {elapsed:?}"
        );
    }
}
