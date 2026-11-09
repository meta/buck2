/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::Command as StdCommand;
use std::process::ExitStatus;

use buck2_resource_control::ActionFreezeEventReceiver;
use buck2_resource_control::path::CgroupPathBuf;
use tokio::io;
use tokio::process::ChildStderr;
use tokio::process::ChildStdout;

#[cfg(unix)]
use crate::unix::process_group as imp;
#[cfg(windows)]
use crate::win::process_group as imp;

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
pub enum SpawnError {
    #[error("Failed to spawn a process: {0}")]
    IoError(io::Error),
    #[error("Failed to create a process group: {0}")]
    GenericError(buck2_error::Error),
}

impl From<buck2_error::Error> for SpawnError {
    #[cold]
    fn from(e: buck2_error::Error) -> Self {
        SpawnError::GenericError(e)
    }
}

impl From<io::Error> for SpawnError {
    #[cold]
    fn from(e: io::Error) -> Self {
        SpawnError::IoError(e)
    }
}

pub(crate) struct ProcessCommand {
    inner: imp::ProcessCommandImpl,
}

impl ProcessCommand {
    pub(crate) async fn new(
        cmd: StdCommand,
        cgroup: Option<CgroupPathBuf>,
    ) -> buck2_error::Result<Self> {
        Ok(Self {
            inner: imp::ProcessCommandImpl::new(cmd, cgroup).await?,
        })
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn spawn(self) -> Result<ProcessGroup, (SpawnError, Self)> {
        match self.inner.spawn() {
            Ok(inner) => Ok(ProcessGroup { inner }),
            Err((e, inner)) => Err((e, Self { inner })),
        }
    }
}

pub(crate) struct ProcessGroup {
    inner: imp::ProcessGroupImpl,
}

impl ProcessGroup {
    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.inner.take_stdout()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.inner.take_stderr()
    }

    pub(crate) async fn wait(
        &mut self,
        freeze_rx: impl ActionFreezeEventReceiver,
    ) -> buck2_error::Result<(ExitStatus, Vec<buck2_resource_control::OrphanProcessInfo>)> {
        self.inner.wait(freeze_rx).await
    }

    pub(crate) fn id(&self) -> Option<u32> {
        self.inner.id()
    }

    pub(crate) async fn kill(
        &mut self,
        graceful_shutdown_timeout_s: Option<u32>,
    ) -> buck2_error::Result<()> {
        self.inner.kill(graceful_shutdown_timeout_s).await
    }
}

#[cfg(test)]
mod tests {
    use buck2_util::process::background_command;

    use crate::process_group::ProcessCommand;

    // The test check basic functionality of process implementation as it differs on Unix and Windows
    #[tokio::test]
    async fn test_process_impl() -> buck2_error::Result<()> {
        let mut cmd;

        if cfg!(windows) {
            cmd = background_command("cmd");
            cmd.arg("/c");
        } else {
            cmd = background_command("sh");
            cmd.arg("-c");
        }
        cmd.arg("exit 2");

        let cmd = ProcessCommand::new(cmd, None).await.unwrap();
        let mut child = cmd.spawn().map_err(|x| x.0).unwrap();

        let id = child.id().expect("missing id");
        assert!(id > 0);

        let (status, _orphans) = child.wait(futures::stream::pending()).await?;
        assert_eq!(status.code(), Some(2));

        // test that the `.wait()` method is fused like tokio
        let (status, _orphans) = child.wait(futures::stream::pending()).await?;
        assert_eq!(status.code(), Some(2));

        // Can't get id after process has exited
        assert_eq!(child.id(), None);
        Ok(())
    }
}
