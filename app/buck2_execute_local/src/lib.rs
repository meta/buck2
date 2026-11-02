/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![cfg_attr(windows, feature(windows_process_extensions_main_thread_handle))]

use std::borrow::Cow;
use std::fs::File;
use std::path::Path;
use std::pin::Pin;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;

use async_trait::async_trait;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_fs::fs_util;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPath;
use buck2_resource_control::ActionFreezeEventReceiver;
use buck2_resource_control::OrphanProcessInfo;
use buck2_resource_control::action_scene::ActionCgroupResult;
use buck2_resource_control::path::CgroupPathBuf;
use bytes::Bytes;
use futures::future::Future;
use futures::future::FutureExt;
use futures::future::select;
use futures::stream::Stream;
use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use pin_project::pin_project;
use tokio_util::codec::BytesCodec;
use tokio_util::codec::FramedRead;

use crate::interruptible_async_read::InterruptNotifiable;
use crate::interruptible_async_read::InterruptibleAsyncRead;
use crate::process_group::ProcessCommand;
use crate::process_group::ProcessGroup;
use crate::process_group::SpawnError;
use crate::status_decoder::DecodedStatus;
use crate::status_decoder::StatusDecoder;

mod interruptible_async_read;
pub mod process_group;
pub mod status_decoder;

#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod win;

#[derive(Debug)]
pub struct CollectedExecutionStats {
    pub cpu_instructions_user: Option<u64>,
    pub cpu_instructions_kernel: Option<u64>,
    pub userspace_events: Option<buck2_data::CpuCounter>,
    pub kernel_events: Option<buck2_data::CpuCounter>,
}

#[derive(Debug)]
pub enum GatherOutputStatus {
    /// Contains the exit code.
    Finished {
        exit_code: i32,
        execution_stats: Option<CollectedExecutionStats>,
    },
    TimedOut(Duration),
    Cancelled,
    SpawnFailed(String),
}

impl From<DecodedStatus> for GatherOutputStatus {
    fn from(d: DecodedStatus) -> Self {
        match d {
            DecodedStatus::Status {
                exit_code,
                execution_stats,
            } => Self::Finished {
                exit_code,
                execution_stats,
            },
            DecodedStatus::SpawnFailed(v) => Self::SpawnFailed(v),
        }
    }
}

#[derive(Debug)]
pub enum CommandEvent {
    Stdout(Bytes),
    Stderr(Bytes),
    Exit(GatherOutputStatus, Vec<OrphanProcessInfo>),
}

enum StdioEvent {
    Stdout(Bytes),
    Stderr(Bytes),
}

impl From<StdioEvent> for CommandEvent {
    fn from(stdio: StdioEvent) -> Self {
        match stdio {
            StdioEvent::Stdout(bytes) => CommandEvent::Stdout(bytes),
            StdioEvent::Stderr(bytes) => CommandEvent::Stderr(bytes),
        }
    }
}

#[derive(Clone)]
pub struct StdRedirectPaths {
    pub stdout: AbsNormPathBuf,
    pub stderr: AbsNormPathBuf,
}

/// This stream will yield [CommandEvent] whenever we have something on stdout or stderr (this is
/// our stdio stream), and it'll finish up the stream with the exit status. This is basically like
/// a select, but with the exit guaranteed to come last.
/// Before sending any events, it will send the cgroup path if cgroup pools are enabled.
#[pin_project]
struct CommandEventStream<Status, Stdio> {
    exit: Option<buck2_error::Result<(GatherOutputStatus, Vec<OrphanProcessInfo>)>>,

    done: bool,

    #[pin]
    status: futures::future::Fuse<Status>,

    #[pin]
    stdio: futures::stream::Fuse<Stdio>,
}

impl<Status, Stdio> CommandEventStream<Status, Stdio>
where
    Status: Future,
    Stdio: Stream,
{
    fn new(status: Status, stdio: Stdio) -> Self {
        Self {
            exit: None,
            done: false,
            status: status.fuse(),
            stdio: stdio.fuse(),
        }
    }
}

impl<Status, Stdio> Stream for CommandEventStream<Status, Stdio>
where
    Status: Future<Output = buck2_error::Result<(GatherOutputStatus, Vec<OrphanProcessInfo>)>>,
    Stdio: Stream<Item = buck2_error::Result<StdioEvent>> + InterruptNotifiable,
{
    type Item = buck2_error::Result<CommandEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if *this.done {
            return Poll::Ready(None);
        }

        // This future is fused so it's guaranteed to be ready once. If it does, capture the exit
        // status, we'll return it later.
        if let Poll::Ready(status) = this.status.poll(cx) {
            *this.exit = Some(status);
            this.stdio.as_mut().get_pin_mut().notify_interrupt();
        }

        // This stream is also fused, so if it returns None, we'll know it's done for good and we'll
        // return the exit status if it's available.
        if let Some(stdio) = futures::ready!(this.stdio.poll_next(cx)) {
            return Poll::Ready(Some(stdio.map(|event| event.into())));
        }

        // If we got here that means the stream is done. If we have it we return, and if we don't
        // we report we're pending, because we'll have polled it already earlier.
        if let Some(exit) = this.exit.take() {
            *this.done = true;
            return Poll::Ready(Some(
                exit.map(|(status, orphans)| CommandEvent::Exit(status, orphans)),
            ));
        }

        Poll::Pending
    }
}

async fn timeout_into_cancellation(
    timeout: Option<Duration>,
) -> buck2_error::Result<GatherOutputStatus> {
    match timeout {
        Some(t) => {
            tokio::time::sleep(t).await;
            Ok(GatherOutputStatus::TimedOut(t))
        }
        None => futures::future::pending().await,
    }
}

#[expect(private_bounds)]
pub async fn spawn_command_and_stream_events<T>(
    mut cmd: Command,
    timeout: Option<Duration>,
    cancellation: T,
    decoder: impl StatusDecoder,
    kill_process: impl KillProcess,
    std_redirects: Option<StdRedirectPaths>,
    retry_on_txt_busy: bool,
    cgroup_path: Option<CgroupPathBuf>,
    freeze_rx: impl ActionFreezeEventReceiver,
) -> buck2_error::Result<impl Stream<Item = buck2_error::Result<CommandEvent>>>
where
    T: Future<Output = buck2_error::Result<GatherOutputStatus>> + Send + Unpin,
{
    cmd.stdin(Stdio::null());
    if let Some(std_redirects) = &std_redirects {
        cmd.stdout(File::create(std_redirects.stdout.as_path())?);
        cmd.stderr(File::create(std_redirects.stderr.as_path())?);
    } else {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
    }

    let cmd = ProcessCommand::new(cmd, cgroup_path).await?;

    // Note: Process spawning is a fundamentally asynchronous operation that legitimately might
    // block for a non-zero amount of time. Typically, that would mean that we should
    // `spawn_blocking` this or similar; however, implementations of process spawning on unix
    // generally use `vfork`, which anyway suspends the entire parent process until the spawn
    // completes. So we don't bother trying anything else here.
    let process_details = if retry_on_txt_busy {
        spawn_retry_txt_busy(cmd, || tokio::time::sleep(Duration::from_millis(50))).await
    } else {
        cmd.spawn().map_err(|x| buck2_error::Error::from(x.0))
    };

    let timeout = timeout_into_cancellation(timeout);

    let cancellation = select(timeout.boxed(), cancellation).map(|r| r.factor_first().0);

    let mut process_group = match process_details {
        Ok(process_group) => process_group,
        Err(e) => {
            let event = Ok(CommandEvent::Exit(
                GatherOutputStatus::SpawnFailed(e.to_string()),
                Vec::new(),
            ));
            return Ok(futures::stream::once(futures::future::ready(event)).left_stream());
        }
    };

    let stdio = if std_redirects.is_none() {
        let stdout = process_group
            .take_stdout()
            .internal_error("Child stdout is not piped")?;
        let stderr = process_group
            .take_stderr()
            .internal_error("Child stderr is not piped")?;

        #[cfg(unix)]
        type Drainer<R> = self::interruptible_async_read::UnixNonBlockingDrainer<R>;

        // On Windows, for the time being we just give ourselves a timeout to finish reading.
        // Ideally this would perform a non-blocking read on self instead like we do on Unix.
        #[cfg(not(unix))]
        type Drainer<R> = self::interruptible_async_read::TimeoutDrainer<R>;

        let stdout = InterruptibleAsyncRead::<_, Drainer<_>>::new(stdout);
        let stderr = InterruptibleAsyncRead::<_, Drainer<_>>::new(stderr);
        let stdout = FramedRead::new(stdout, BytesCodec::new())
            .map(|data| Ok(StdioEvent::Stdout(data?.freeze())));
        let stderr = FramedRead::new(stderr, BytesCodec::new())
            .map(|data| Ok(StdioEvent::Stderr(data?.freeze())));

        futures::stream::select(stdout, stderr).left_stream()
    } else {
        futures::stream::empty().right_stream()
    };

    let status = async move {
        enum Outcome {
            Finished(ExitStatus, Vec<OrphanProcessInfo>),
            Cancelled(GatherOutputStatus),
        }

        // NOTE: This wrapping here is so that we release the borrow of `child` that stems from
        // `wait()` by the time we call kill_process a few lines down.
        let execute = async {
            let status = process_group.wait(freeze_rx);
            futures::pin_mut!(status);
            futures::pin_mut!(cancellation);

            buck2_error::Ok(match futures::future::select(status, cancellation).await {
                futures::future::Either::Left((result, _)) => {
                    let (status, orphans) = result?;
                    Outcome::Finished(status, orphans)
                }
                futures::future::Either::Right((res, _)) => Outcome::Cancelled(res?),
            })
        };

        let (gather_output_status, orphans) = match execute.await? {
            Outcome::Finished(status, orphans) => {
                (decoder.decode_status(status).await?.into(), orphans)
            }
            Outcome::Cancelled(res) => {
                kill_process
                    .kill(&mut process_group)
                    .await
                    .buck_error_context("Failed to terminate child after timeout")?;

                decoder
                    .cancel()
                    .await
                    .buck_error_context("Failed to cancel status decoder after timeout")?;

                // We just killed the child, so this should finish immediately. We should still call
                // this to release any process.
                let (_status, orphans) = process_group
                    .wait(futures::stream::pending())
                    .await
                    .buck_error_context("Failed to await child after kill")?;

                (res, orphans)
            }
        };

        Ok((gather_output_status, orphans))
    };

    Ok(CommandEventStream::new(status, stdio).right_stream())
}

pub struct CommandResult {
    pub status: GatherOutputStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub cgroup_result: Option<ActionCgroupResult>,
    pub orphan_processes: Vec<OrphanProcessInfo>,
}

pub async fn decode_command_event_stream<S>(stream: S) -> buck2_error::Result<CommandResult>
where
    S: Stream<Item = buck2_error::Result<CommandEvent>>,
{
    futures::pin_mut!(stream);

    let mut stdout = Vec::<u8>::new();
    let mut stderr = Vec::<u8>::new();

    while let Some(event) = stream.try_next().await? {
        match event {
            CommandEvent::Stdout(bytes) => stdout.extend(&bytes),
            CommandEvent::Stderr(bytes) => stderr.extend(&bytes),
            CommandEvent::Exit(exit, orphan_processes) => {
                return Ok(CommandResult {
                    status: exit,
                    stdout,
                    stderr,
                    // Filled in later
                    cgroup_result: None,
                    orphan_processes,
                });
            }
        }
    }

    Err(buck2_error::buck2_error!(
        buck2_error::ErrorTag::Tier0,
        "Stream did not yield CommandEvent::Exit",
    ))
}

/// Dependency injection for kill. We use this in testing.
#[async_trait]
pub(crate) trait KillProcess {
    async fn kill(self, process: &mut ProcessGroup) -> buck2_error::Result<()>;
}

#[derive(Default)]
pub struct DefaultKillProcess {
    pub graceful_shutdown_timeout_s: Option<u32>,
}

#[async_trait]
impl KillProcess for DefaultKillProcess {
    async fn kill(self, process_group: &mut ProcessGroup) -> buck2_error::Result<()> {
        let pid = match process_group.id() {
            Some(pid) => pid,
            None => {
                // Child just exited, so in this case we don't want to kill anything.
                return Ok(());
            }
        };
        tracing::info!("Killing process {}", pid);
        process_group.kill(self.graceful_shutdown_timeout_s).await
    }
}

/// Unify the behavior of using a relative path for the executable between Unix and Windows. On
/// UNIX, the path is understood to be relative to the cwd of the *spawned process*, whereas on
/// Windows, it's relative ot the cwd of the *spawning* process.
///
/// Here, we unify the two behaviors since we always run our subprocesses with a known cwd: we
/// check if the executable actually exists relative to said cwd, and if it does, we use that.
pub fn maybe_absolutize_exe<'a>(
    exe: &'a (impl AsRef<Path> + ?Sized),
    spawned_process_cwd: &'_ AbsPath,
) -> buck2_error::Result<Cow<'a, Path>> {
    let exe = exe.as_ref();

    let abs = spawned_process_cwd.join(exe);
    if fs_util::try_exists(&abs).buck_error_context("Error absolute-izing executable")? {
        return Ok(abs.into_path_buf().into());
    }

    Ok(exe.into())
}

/// fork-exec is a bit tricky in a busy process. We often have files open to writing just prior to
/// executing them (as we download from RE), and many processes being spawned concurrently. We do
/// close the fds properly before the exec, but what can happn is:
///
/// - Some thread forks
///
/// - We close the file. At this time we don't have it open, but the forked process does.
///
/// - We try to exec the file. This fails because the file is open for writing (by the forked
///   process).
///
/// - The forked process execs. At this point the file is closed (because everything is CLOEXEC).
///
/// The window during which the forked process holds the fd is small, so retrying a couple times
/// here should let us make this work.
///
/// The more correct solution for this here would be to start a fork server in a separate process
/// when we start.  However, until we get there, this should do the trick.
async fn spawn_retry_txt_busy<F, D>(
    mut cmd: ProcessCommand,
    mut delay: F,
) -> buck2_error::Result<ProcessGroup>
where
    F: FnMut() -> D,
    D: Future<Output = ()>,
{
    let mut attempts = 10;

    loop {
        let err = match cmd.spawn() {
            Ok(pg) => return Ok(pg),
            Err((e, cmd_back)) => {
                cmd = cmd_back;
                e
            }
        };

        let is_txt_busy = match &err {
            SpawnError::IoError(e) => matches!(e.raw_os_error(), Some(libc::ETXTBSY)),
            SpawnError::GenericError(_) => false,
        };

        if attempts == 0 || !is_txt_busy {
            return Err(err.into());
        }

        delay().await;

        attempts -= 1;
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::str;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Instant;

    use assert_matches::assert_matches;
    use buck2_error::buck2_error;
    use buck2_util::process::background_command;
    use dupe::Dupe;

    use super::*;
    use crate::status_decoder::DefaultStatusDecoder;

    async fn gather_output(
        cmd: Command,
        timeout: Option<Duration>,
    ) -> buck2_error::Result<CommandResult> {
        let stream = spawn_command_and_stream_events(
            cmd,
            timeout,
            futures::future::pending(),
            DefaultStatusDecoder,
            DefaultKillProcess::default(),
            None,
            true,
            None,
            futures::stream::pending(),
        )
        .await?;
        decode_command_event_stream(stream).await
    }

    #[tokio::test]
    async fn test_gather_output() -> buck2_error::Result<()> {
        let mut cmd = if cfg!(windows) {
            background_command("powershell")
        } else {
            background_command("sh")
        };
        cmd.args(["-c", "echo hello"]);

        let CommandResult {
            status,
            stdout,
            stderr,
            ..
        } = gather_output(cmd, None).await?;
        assert!(matches!(status, GatherOutputStatus::Finished { exit_code, .. } if exit_code == 0));
        assert_eq!(str::from_utf8(&stdout)?.trim(), "hello");
        assert_eq!(stderr, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_gather_does_not_wait_for_children() -> buck2_error::Result<()> {
        // If we wait for sleep, this will time out.
        let mut cmd = if cfg!(windows) {
            // Use cmd.exe rather than PowerShell — PowerShell's Start-Job is
            // heavyweight and can exceed the timeout on slow CI machines.
            background_command("cmd")
        } else {
            background_command("sh")
        };
        if cfg!(windows) {
            // `ping` to loopback is the sleep: `timeout` would be GNU coreutils' when Git's
            // `usr/bin` precedes System32 on PATH, which has no command to run and says so on
            // stderr.
            cmd.args(["/c", "start /b ping -n 11 127.0.0.1 >nul & echo hello"]);
        } else {
            cmd.args(["-c", "(sleep 10 &) && echo hello"]);
        }

        let timeout = if cfg!(windows) { 9 } else { 1 };
        let CommandResult {
            status,
            stdout,
            stderr,
            ..
        } = gather_output(cmd, Some(Duration::from_secs(timeout))).await?;
        assert!(
            matches!(status, GatherOutputStatus::Finished { exit_code, .. } if exit_code == 0),
            "status: {status:?}"
        );
        assert_eq!(str::from_utf8(&stdout)?.trim(), "hello");
        assert_eq!(stderr, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_gather_output_timeout() -> buck2_error::Result<()> {
        let now = Instant::now();

        let cmd = if cfg!(windows) {
            let mut cmd = background_command("powershell");
            cmd.args(["-c", "echo hello; sleep 10; echo bye"]);
            cmd
        } else {
            let mut cmd = background_command("sh");
            cmd.args(["-c", "echo hello && sleep 10 && echo bye"]);
            cmd
        };

        let timeout = if cfg!(windows) { 5 } else { 3 };
        let CommandResult { status, stdout, .. } =
            gather_output(cmd, Some(Duration::from_secs(timeout))).await?;
        assert!(
            matches!(status, GatherOutputStatus::TimedOut(..)),
            "status: {status:?}"
        );
        assert_eq!(str::from_utf8(&stdout)?.trim(), "hello");
        // Do not check stderr because stderr may contain a message like:
        // ```
        // sh: line 1: 41348 Killed: 9
        // ```
        // or it can be empty, which depends on which process is killed first by killpg.

        assert!(Instant::now() - now < Duration::from_secs(9)); // Lots of leeway here.

        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_spawn_retry_txt_busy() -> buck2_error::Result<()> {
        use tokio::fs::OpenOptions;
        use tokio::io::AsyncWriteExt;

        let tempdir = tempfile::tempdir()?;
        let bin = tempdir.path().join("bin");

        let mut file = OpenOptions::new()
            .mode(0o755)
            .write(true)
            .truncate(true)
            .create(true)
            .open(&bin)
            .await?;

        file.write_all(b"#!/usr/bin/env bash\ntrue\n").await?;

        let cmd = background_command(&bin);
        let cmd = ProcessCommand::new(cmd, None).await?;
        // Sibling tests fork concurrently and hold the inherited write handle
        // until they exec, so dropping ours does not clear ETXTBSY by itself.
        let mut process_group = spawn_retry_txt_busy(cmd, {
            let mut file = Some(file);
            move || {
                file.take();
                tokio::time::sleep(Duration::from_millis(10))
            }
        })
        .await?;

        let (status, _orphans) = process_group.wait(futures::stream::pending()).await?;
        assert_eq!(status.code(), Some(0));

        Ok(())
    }

    #[tokio::test]
    async fn test_spawn_retry_other_error() -> buck2_error::Result<()> {
        let tempdir = tempfile::tempdir()?;
        let bin = tempdir.path().join("bin"); // Does not actually exist

        let cmd = background_command(&bin);
        let cmd = ProcessCommand::new(cmd, None).await?;
        let res = spawn_retry_txt_busy(cmd, || async { panic!("Should not be called!") }).await;
        assert!(res.is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_kill_terminates_process_group() -> buck2_error::Result<()> {
        use sysinfo::Pid;
        use sysinfo::System;

        let cmd = if cfg!(windows) {
            let mut cmd = background_command("powershell");
            cmd.args([
                "-c",
                "echo $PID; Start-Process -FilePath \"powershell\" -Wait -NoNewWindow -ArgumentList \
                'echo $PID; Sleep 1000'",
            ]);
            cmd
        } else {
            let mut cmd = background_command("sh");
            cmd.arg("-c")
                .arg("( ( echo $PPID && echo $$ && sleep 1000 ) )");
            cmd
        };

        // On windows we need more time to run powershell
        let timeout = if cfg!(windows) { 7 } else { 1 };
        let CommandResult { stdout, .. } =
            gather_output(cmd, Some(Duration::from_secs(timeout))).await?;
        let out = str::from_utf8(&stdout)?;
        let pids: Vec<&str> = out.split('\n').collect();
        let ppid = Pid::from_str(pids.first().internal_error("no ppid")?.trim())?;
        let pid = Pid::from_str(pids.get(1).internal_error("no pid")?.trim())?;
        let sys = System::new_all();

        // we want to check if existed process doesn't have the same parent because of pid reuse
        if let Some(process) = sys.process(pid) {
            if let Some(parent) = process.parent() {
                if parent != ppid {
                    return Ok(());
                }
            }
            return Err(buck2_error!(
                buck2_error::ErrorTag::Tier0,
                "PID still exits: {}",
                pid
            ));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_stream_command_events_ends() -> buck2_error::Result<()> {
        let mut cmd = if cfg!(windows) {
            background_command("powershell")
        } else {
            background_command("sh")
        };
        cmd.args(["-c", "exit 0"]);

        let mut events = spawn_command_and_stream_events(
            cmd,
            None,
            futures::future::pending(),
            DefaultStatusDecoder,
            DefaultKillProcess::default(),
            None,
            true,
            None,
            futures::stream::pending(),
        )
        .await?
        .boxed();
        assert_matches!(events.next().await, Some(Ok(CommandEvent::Exit(..))));
        assert_matches!(futures::poll!(events.next()), Poll::Ready(None));
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_signal_exit_code() -> buck2_error::Result<()> {
        use nix::sys::signal::Signal;

        let mut cmd = background_command("sh");
        cmd.arg("-c").arg("kill -KILL \"$$\"");
        let CommandResult { status, .. } = gather_output(cmd, None).await?;

        assert_matches!(
            status,
            GatherOutputStatus::Finished { exit_code, .. } if exit_code == 128 + Signal::SIGKILL as i32
        );

        Ok(())
    }

    #[tokio::test]
    async fn timeout_kills_before_dropping_decoder() -> buck2_error::Result<()> {
        struct Kill {
            killed: Arc<Mutex<bool>>,
        }

        #[async_trait]
        impl KillProcess for Kill {
            async fn kill(self, process_group: &mut ProcessGroup) -> buck2_error::Result<()> {
                *self.killed.lock().unwrap() = true;

                // We still need to kill the process. On Windows in particular our test will hang
                // if we do not.
                DefaultKillProcess::default().kill(process_group).await
            }
        }

        struct Decoder {
            killed: Arc<Mutex<bool>>,
            cancelled: Arc<Mutex<bool>>,
        }

        #[async_trait::async_trait]
        impl StatusDecoder for Decoder {
            async fn decode_status(
                self,
                _status: ExitStatus,
            ) -> buck2_error::Result<DecodedStatus> {
                panic!("Should not be called in this test since we timeout")
            }

            async fn cancel(self) -> buck2_error::Result<()> {
                assert!(*self.killed.lock().unwrap());
                *self.cancelled.lock().unwrap() = true;
                Ok(())
            }
        }

        let killed = Arc::new(Mutex::new(false));
        let cancelled = Arc::new(Mutex::new(false));

        let mut cmd = if cfg!(windows) {
            background_command("powershell")
        } else {
            background_command("sh")
        };
        cmd.args(["-c", "sleep 10000"]);

        let stream = spawn_command_and_stream_events(
            cmd,
            Some(Duration::from_secs(1)),
            futures::future::pending(),
            Decoder {
                killed: killed.dupe(),
                cancelled: cancelled.dupe(),
            },
            Kill {
                killed: killed.dupe(),
            },
            None,
            true,
            None,
            futures::stream::pending(),
        )
        .await?;

        let CommandResult { status, .. } = decode_command_event_stream(stream).await?;
        assert!(matches!(status, GatherOutputStatus::TimedOut(..)));

        assert!(*killed.lock().unwrap());
        assert!(*cancelled.lock().unwrap());

        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_no_stdio_stream_command_events() -> buck2_error::Result<()> {
        let mut cmd = background_command("sh");
        cmd.args(["-c", "echo hello"]);

        let tempdir = tempfile::tempdir()?;
        let stdout = AbsNormPathBuf::new(tempdir.path().join("stdout")).unwrap();
        let stderr = AbsNormPathBuf::new(tempdir.path().join("stderr")).unwrap();

        let mut events = spawn_command_and_stream_events(
            cmd,
            None,
            futures::future::pending(),
            DefaultStatusDecoder,
            DefaultKillProcess::default(),
            Some(StdRedirectPaths {
                stdout: stdout.clone(),
                stderr: stderr.clone(),
            }),
            false,
            None,
            futures::stream::pending(),
        )
        .await?
        .boxed();
        assert_matches!(events.next().await, Some(Ok(CommandEvent::Exit(..))));
        assert_matches!(futures::poll!(events.next()), Poll::Ready(None));

        assert_matches!(tokio::fs::read_to_string(stdout).await?.as_str(), "hello\n");

        Ok(())
    }
}
