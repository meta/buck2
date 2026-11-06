/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! buck2 commands that a test talks to while they run, through their stdin and stdout, like
//! `buck2 subscribe` and `buck2 lsp`.

use std::ffi::OsString;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::ChildStdin;
use tokio::process::ChildStdout;

use crate::buck::BuckCommand;
use crate::buck::read_all;
use crate::output::BuckError;
use crate::output::BuckOutput;

/// A started buck2 command whose stdin the test writes to and whose stdout a client reads; its
/// stderr is collected meanwhile. Dropping it kills the process.
pub struct PipedProcess {
    child: tokio::process::Child,
    stdin: Option<ChildStdin>,
    stderr: tokio::task::JoinHandle<Vec<u8>>,
    command: Vec<OsString>,
    args: Vec<String>,
    working_dir: PathBuf,
    invocation_record_path: Option<PathBuf>,
}

impl BuckCommand<'_> {
    /// Starts the command with its stdin and stdout piped to the caller.
    pub(crate) fn spawn_piped(self) -> (PipedProcess, ChildStdout) {
        let invocation_id = uuid::Uuid::new_v4().to_string();
        let prepared = self.prepare(&invocation_id);
        assert!(
            prepared.cwd.exists(),
            "The working directory `{}` does not exist",
            prepared.cwd.display()
        );
        if let Some(parent) = prepared
            .invocation_record_path
            .as_deref()
            .and_then(Path::parent)
        {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("Could not create `{}`: {e}", parent.display()));
        }

        let mut child = tokio::process::Command::new(&prepared.argv[0])
            .args(&prepared.argv[1..])
            .current_dir(&prepared.cwd)
            .env_clear()
            .envs(&prepared.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|e| {
                panic!(
                    "Could not run `{}`: {e}",
                    prepared.argv[0].to_string_lossy()
                )
            });
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = read_all(child.stderr.take().expect("stderr is piped"));
        let process = PipedProcess {
            child,
            stdin: Some(stdin),
            stderr,
            command: prepared.argv,
            args: prepared
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect(),
            working_dir: prepared.cwd,
            invocation_record_path: prepared.invocation_record_path,
        };
        (process, stdout)
    }
}

impl PipedProcess {
    /// The process id, until the process has been waited for.
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }

    /// Writes `data` to the process's stdin and flushes it. Panics if stdin is closed.
    pub async fn write_stdin(&mut self, data: &[u8]) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .expect("stdin is open: `close_stdin` was not called");
        stdin.write_all(data).await?;
        stdin.flush().await
    }

    /// Closes stdin, so that the process reads EOF.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// The exit status, if the process has exited.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .try_wait()
            .unwrap_or_else(|e| panic!("Could not check on the process: {e}"))
    }

    /// Waits up to `timeout` for the process to exit, giving its exit status if it did.
    pub async fn wait_timeout(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let status = tokio::time::timeout(timeout, self.child.wait())
            .await
            .ok()?;
        Some(status.unwrap_or_else(|e| panic!("Could not wait for the process: {e}")))
    }

    /// Kills the process unless it has exited, then waits up to `timeout` for it. Panics if it does
    /// not exit in time.
    pub async fn kill(&mut self, timeout: Duration) -> ExitStatus {
        if let Some(status) = self.try_wait() {
            return status;
        }
        // It may exit meanwhile.
        let _ignored = self.child.start_kill();
        self.wait_timeout(timeout).await.unwrap_or_else(|| {
            panic!(
                "`{}` did not exit within {timeout:?} of being killed",
                self.command_line()
            )
        })
    }

    /// Closes stdin and waits up to `timeout` for the process to exit and for `stdout`, the rest of
    /// what the client read, giving the command's [`BuckOutput`] or, if buck2 exited
    /// unsuccessfully, a [`BuckError`]. Panics if that takes longer than `timeout`.
    pub(crate) async fn finish(
        mut self,
        stdout: impl Future<Output = String>,
        timeout: Duration,
    ) -> Result<BuckOutput, BuckError> {
        self.close_stdin();
        let command_line = self.command_line();
        let child = &mut self.child;
        let stderr = &mut self.stderr;
        let (status, stdout, stderr) = tokio::time::timeout(timeout, async move {
            let status = child
                .wait()
                .await
                .unwrap_or_else(|e| panic!("Could not wait for the process: {e}"));
            let stdout = stdout.await;
            let stderr = stderr.await.expect("reading stderr");
            (status, stdout, stderr)
        })
        .await
        .unwrap_or_else(|_| panic!("`{command_line}` did not finish within {timeout:?}"));
        let output = BuckOutput {
            stdout,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            status,
            command: std::mem::take(&mut self.command),
            args: std::mem::take(&mut self.args),
            working_dir: std::mem::take(&mut self.working_dir),
            invocation_record_path: self.invocation_record_path.take(),
        };
        if output.status.success() {
            Ok(output)
        } else {
            Err(BuckError(Box::new(output)))
        }
    }

    fn command_line(&self) -> String {
        self.command
            .iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;

    use tokio::io::AsyncBufReadExt;
    use tokio::io::BufReader;

    use super::*;
    use crate::Buck;

    /// A [`Buck`] whose "buck2" is the shell, so that `command(&["-c", script])` runs `script`.
    fn shell(dir: &Path) -> Buck {
        Buck::new(
            PathBuf::from("/bin/sh"),
            dir.to_owned(),
            BTreeMap::new(),
            false,
        )
    }

    #[tokio::test]
    async fn test_the_test_talks_to_the_process_through_its_pipes() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let buck = shell(dir.path());
        let (mut process, stdout) = buck
            .command(&["-c", "read line; echo \"got $line\""])
            .spawn_piped();
        let mut stdout = BufReader::new(stdout);

        process.write_stdin(b"hello\n").await?;
        let mut line = String::new();
        stdout.read_line(&mut line).await?;
        assert_eq!(line, "got hello\n");

        let output = process
            .finish(async { String::new() }, Duration::from_secs(30))
            .await?;
        assert_eq!(output.exit_code(), Some(0));
        Ok(())
    }

    #[tokio::test]
    async fn test_finishing_closes_stdin() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let buck = shell(dir.path());
        // `cat` exits only at EOF.
        let (process, _stdout) = buck.command(&["-c", "cat >/dev/null"]).spawn_piped();
        process
            .finish(async { String::new() }, Duration::from_secs(30))
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_an_unsuccessful_exit_is_an_error_with_the_output() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let buck = shell(dir.path());
        let (process, _stdout) = buck.command(&["-c", "echo oops >&2; exit 3"]).spawn_piped();
        let error = process
            .finish(async { "the rest\n".to_owned() }, Duration::from_secs(30))
            .await
            .expect_err("the command exits with 3");
        assert_eq!(error.exit_code(), Some(3));
        assert_eq!(error.stderr, "oops\n");
        assert_eq!(error.stdout, "the rest\n");
        Ok(())
    }

    #[tokio::test]
    async fn test_a_running_process_can_be_waited_for_and_killed() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let buck = shell(dir.path());
        let (mut process, _stdout) = buck.command(&["-c", "sleep 600"]).spawn_piped();
        assert!(process.try_wait().is_none(), "the process is still running");
        assert!(
            process
                .wait_timeout(Duration::from_millis(100))
                .await
                .is_none(),
            "the process does not exit by itself"
        );
        let status = process.kill(Duration::from_secs(30)).await;
        assert!(!status.success(), "a killed process does not succeed");
        assert!(process.try_wait().is_some());
        Ok(())
    }
}
