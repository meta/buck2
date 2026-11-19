/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::ChildExt;
use std::os::windows::process::CommandExt;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::time::Duration;

use buck2_error::BuckErrorOptionContext;
use buck2_resource_control::ActionFreezeEventReceiver;
use buck2_resource_control::path::CgroupPathBuf;
use tokio::process::ChildStderr;
use tokio::process::ChildStdout;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
use windows_sys::Win32::System::Threading::ResumeThread;

use crate::process_group::SpawnError;
use crate::win::child_process::ChildProcess;
use crate::win::job_object::JobObject;
use crate::win::utils::result_dword;

pub(crate) struct ProcessCommandImpl {
    inner: Command,
}

impl ProcessCommandImpl {
    pub(crate) async fn new(
        mut cmd: Command,
        _cgroup: Option<CgroupPathBuf>,
        // The job object already kills every process the command started.
        _kill_detached_descendants: bool,
    ) -> buck2_error::Result<Self> {
        // On windows we create suspended process to assign it to a job (group) and then resume.
        // This is necessary because the process might finish before we add it to a job
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
        Ok(Self { inner: cmd })
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn spawn(mut self) -> Result<ProcessGroupImpl, (SpawnError, Self)> {
        match self.inner.spawn() {
            Ok(inner) => match ProcessGroupImpl::new(inner) {
                Ok(inner) => Ok(inner),
                Err(e) => Err((e.into(), self)),
            },
            Err(e) => Err((e.into(), self)),
        }
    }
}

/// Keeps track of the exit status of a child process without worrying about
/// polling the underlying futures even after they have completed.
enum FusedChild {
    Child(Box<ChildProcess>),
    Done(ExitStatus),
}

impl FusedChild {
    fn as_option(&self) -> Option<&ChildProcess> {
        match &self {
            FusedChild::Child(child) => Some(child),
            FusedChild::Done(_) => None,
        }
    }

    fn as_option_mut(&mut self) -> Option<&mut ChildProcess> {
        match self {
            FusedChild::Child(child) => Some(child),
            FusedChild::Done(_) => None,
        }
    }
}

pub(crate) struct ProcessGroupImpl {
    child: FusedChild,
    job: JobObject,
}

impl ProcessGroupImpl {
    fn new(child: Child) -> buck2_error::Result<ProcessGroupImpl> {
        let job = JobObject::new()?;
        job.assign_process(child.as_raw_handle() as HANDLE)?;
        let process = ProcessGroupImpl {
            child: FusedChild::Child(Box::new(ChildProcess::new(child))),
            job,
        };
        // We create suspended process to assign it to a job (group)
        // So we resume the process after assignment
        process.resume()?;
        Ok(process)
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child
            .as_option_mut()?
            .as_std_mut()
            .stdout
            .take()
            .and_then(|s| ChildStdout::from_std(s).ok())
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child
            .as_option_mut()?
            .as_std_mut()
            .stderr
            .take()
            .and_then(|s| ChildStderr::from_std(s).ok())
    }

    pub(crate) async fn wait(
        &mut self,
        _freeze_rx: impl ActionFreezeEventReceiver,
    ) -> buck2_error::Result<(ExitStatus, Vec<buck2_resource_control::OrphanProcessInfo>)> {
        match &mut self.child {
            FusedChild::Done(exit) => Ok((*exit, Vec::new())),
            FusedChild::Child(child) => {
                // Ensure stdin is closed so the child isn't stuck waiting on
                // input while the parent is waiting for it to exit.
                drop(child.as_std_mut().stdin.take());
                let ret = child.await;

                if let Ok(exit) = ret {
                    self.child = FusedChild::Done(exit);
                }

                ret.map(|status| (status, Vec::new())).map_err(Into::into)
            }
        }
    }

    pub(crate) fn id(&self) -> Option<u32> {
        Some(self.child.as_option()?.as_std().id())
    }

    // On Windows we use JobObject API to kill the whole process tree
    pub(crate) async fn kill(
        &mut self,
        _graceful_shutdown_timeout_s: Option<u32>,
    ) -> buck2_error::Result<()> {
        tokio::time::timeout(Duration::from_secs(10), self.job.terminate(0))
            .await
            .map_err(|_| {
                buck2_error::buck2_error!(
                    buck2_error::ErrorTag::Tier0,
                    "Timed out on job object termination"
                )
            })?
    }

    fn resume(&self) -> buck2_error::Result<()> {
        let handle = self
            .child
            .as_option()
            .internal_error("Can't resume an exited process")?
            .as_std()
            .main_thread_handle()
            .as_raw_handle();
        result_dword(unsafe { ResumeThread(handle as HANDLE) })
    }
}
