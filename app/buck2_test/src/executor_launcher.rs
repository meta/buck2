/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::task::Context;
use std::task::Poll;

use async_trait::async_trait;
use buck2_core::buck2_env;
use buck2_error::BuckErrorContext as _;
use buck2_events::dispatch::EventDispatcher;
use buck2_grpc::DuplexChannel;
use buck2_grpc::ServerHandle;
use buck2_hash::BuckMutMap;
use buck2_test_api::grpc::TestExecutorClient;
use buck2_test_api::grpc::spawn_orchestrator_server;
use buck2_test_api::protocol::TestExecutor;
use dupe::Dupe;
use futures::future::BoxFuture;
use futures::future::Future;
use futures::future::FutureExt;
use futures::future::try_join3;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::process::Child;

use crate::downward_api::BuckTestDownwardApi;
use crate::orchestrator::BuckTestOrchestrator;

static TEST_EXECUTOR_CLIENTS: LazyLock<Mutex<BuckMutMap<u16, Arc<dyn TestExecutor>>>> =
    LazyLock::new(|| Mutex::new(BuckMutMap::default()));

pub struct TestExecutorClientWrapper(u16);
impl TestExecutorClientWrapper {
    pub fn new(client: Arc<dyn TestExecutor>) -> Self {
        let mut clients = TEST_EXECUTOR_CLIENTS.lock().unwrap();
        let id = clients.keys().max().unwrap_or(&0) + 1;
        tracing::debug!(id = id, "Adding test executor");
        clients.insert(id, client);
        Self(id)
    }
}
impl Drop for TestExecutorClientWrapper {
    fn drop(&mut self) {
        tracing::debug!(id = self.0, "Removing test executor");
        TEST_EXECUTOR_CLIENTS.lock().unwrap().remove(&self.0);
    }
}

pub fn get_all_test_executors() -> Vec<Arc<dyn TestExecutor>> {
    TEST_EXECUTOR_CLIENTS
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect()
}

pub struct ExecutorLaunch {
    pub handle: ExecutorFuture,
    pub client: TestExecutorClient,
    pub make_server:
        Box<dyn FnOnce(BuckTestOrchestrator<'static>, BuckTestDownwardApi) -> ServerHandle + Send>,
}

pub struct ExecutorFuture {
    fut: BoxFuture<'static, buck2_error::Result<ExecutorOutput>>,
}

impl ExecutorFuture {
    pub(crate) fn new(mut child: Child) -> Self {
        let fut = async move {
            let stdout_fut = read_and_log::read_to_end("stdout", child.stdout.take());
            let stderr_fut = read_and_log::read_to_end("stderr", child.stderr.take());

            let (status, stdout, stderr) = try_join3(child.wait(), stdout_fut, stderr_fut)
                .await
                .buck_error_context("Failed to run OutOfProcessTestExecutor")?;

            // Preserve the distinction between an orderly non-zero exit and a signal
            // death (crash or OOM kill): `code()` is `None` when the process was
            // terminated by a signal, so coercing it to an exit code would make a real
            // crash indistinguishable from a clean failure.
            let exit_code = status.code();

            #[cfg(unix)]
            let (signal, core_dumped) = (status.signal(), status.core_dumped());
            #[cfg(not(unix))]
            let (signal, core_dumped) = (None, false);

            Ok(ExecutorOutput {
                exit_code,
                signal,
                core_dumped,
                stdout,
                stderr,
            })
        };

        Self { fut: fut.boxed() }
    }
}

impl Future for ExecutorFuture {
    type Output = buck2_error::Result<ExecutorOutput>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.fut.poll_unpin(cx)
    }
}

#[derive(Debug)]
pub struct ExecutorOutput {
    /// The process exit code, or `None` if the process was terminated by a signal.
    pub exit_code: Option<i32>,
    /// The signal that terminated the process, if it was terminated by one (Unix only).
    pub signal: Option<i32>,
    /// Whether the terminated process dumped core (Unix only).
    pub core_dumped: bool,
    pub stdout: String,
    pub stderr: String,
}

impl ExecutorOutput {
    /// Human-readable description of how the test executor terminated, including its
    /// captured stdout/stderr, for the error surfaced when it does not exit cleanly.
    pub(crate) fn termination_message(&self) -> String {
        let how = match (self.signal, self.exit_code) {
            (Some(signal), _) => {
                let core = if self.core_dumped {
                    " (core dumped)"
                } else {
                    ""
                };
                format!("was terminated by signal {signal}{core}")
            }
            (None, Some(exit_code)) => format!("exited unexpectedly with status {exit_code}"),
            (None, None) => "exited unexpectedly with unknown status".to_owned(),
        };
        format!(
            "Test executor {how}.\nStdout:\n{}\nStderr:\n{}",
            self.stdout, self.stderr
        )
    }
}

#[async_trait]
pub trait ExecutorLauncher: Send + Sync {
    async fn launch(&self, tpx_args: Vec<String>) -> buck2_error::Result<ExecutorLaunch>;
}

pub struct OutOfProcessTestExecutor {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub dispatcher: EventDispatcher,
}

#[async_trait]
impl ExecutorLauncher for OutOfProcessTestExecutor {
    async fn launch(&self, tpx_args: Vec<String>) -> buck2_error::Result<ExecutorLaunch> {
        // Declare outside of `cfg(unix)` so `buck2 help-env` would include it on Windows
        // even if it is no-op on Windows.
        let use_tcp = buck2_env!("BUCK2_TEST_TPX_USE_TCP", bool)?;

        if !use_tcp {
            #[cfg(unix)]
            {
                return spawn_orchestrator(
                    crate::unix::executor::spawn(
                        self.executable.as_ref(),
                        self.args.clone(),
                        tpx_args,
                    )
                    .await?,
                    self.dispatcher.dupe(),
                )
                .await;
            }
        }

        spawn_orchestrator(
            crate::tcp::executor::spawn(self.executable.as_ref(), self.args.clone(), tpx_args)
                .await?,
            self.dispatcher.dupe(),
        )
        .await
    }
}
async fn spawn_orchestrator<T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static>(
    (handle, executor_client_io, orchestrator_server_io): (ExecutorFuture, T, T),
    dispatcher: EventDispatcher,
) -> buck2_error::Result<ExecutorLaunch> {
    let client = TestExecutorClient::new(executor_client_io)
        .await
        .buck_error_context("Failed to create TestExecutorClient")?;

    let make_server = Box::new(move |orchestrator, downward_api| {
        let (read, write) = tokio::io::split(orchestrator_server_io);
        let orchestrator_server_io = DuplexChannel::new(read, write);
        spawn_orchestrator_server(
            orchestrator_server_io,
            orchestrator,
            downward_api,
            dispatcher,
        )
    });

    Ok(ExecutorLaunch {
        handle,
        client,
        make_server,
    })
}

mod read_and_log {
    use std::io;

    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncRead;
    use tokio::io::BufReader;

    pub async fn read_to_end<A: AsyncRead + Unpin>(
        channel: &str,
        io: Option<A>,
    ) -> io::Result<String> {
        let mut ret = Vec::new();

        if let Some(io) = io {
            let reader = BufReader::new(io);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await? {
                tracing::debug!(channel = channel, "{}", line);
                ret.push(line);
            }
        }

        // Get a trailing newline.
        ret.push("".into());

        Ok(ret.join("\n"))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use buck2_util::process::async_background_command;

    use super::*;

    // A live signal death (as with a crash or an OOM kill) must be reported as a
    // signal, not coerced into an exit code.
    #[tokio::test]
    async fn test_signal_death_is_captured() {
        let child = async_background_command("sh")
            .arg("-c")
            .arg("kill -KILL $$")
            .spawn()
            .unwrap();

        let output = ExecutorFuture::new(child).await.unwrap();

        assert_eq!(output.exit_code, None);
        assert_eq!(output.signal, Some(9));
        assert!(
            output
                .termination_message()
                .contains("was terminated by signal 9"),
            "unexpected message: {}",
            output.termination_message()
        );
    }

    // An orderly non-zero exit must be reported as an exit code, with no signal.
    #[tokio::test]
    async fn test_non_zero_exit_is_captured() {
        let child = async_background_command("sh")
            .arg("-c")
            .arg("exit 3")
            .spawn()
            .unwrap();

        let output = ExecutorFuture::new(child).await.unwrap();

        assert_eq!(output.signal, None);
        assert_eq!(output.exit_code, Some(3));
        assert!(
            output
                .termination_message()
                .contains("exited unexpectedly with status 3"),
            "unexpected message: {}",
            output.termination_message()
        );
    }
}
