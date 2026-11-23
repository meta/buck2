/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Starts the helper for each command that carries event forwarding keys, and writes the
//! command's event log to it, off the command's path.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use buck2_cli_proto::CommandResult;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_server::command_observer::CommandLifecycleObserver;
use buck2_server::command_observer::CommandObserver;
use buck2_server::command_observer::StartedCommand;
use dupe::Dupe;
use futures::FutureExt;
use futures::future::BoxFuture;
use tokio::io::AsyncWrite;
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::encode;
use crate::helper::Helper;
use crate::helper::LaunchHelper;

/// How long after its command ended the command's result may still be written, to a helper that
/// is slow to read.
const WRITE_AFTER_END: Duration = Duration::from_secs(60);
/// How long a helper may keep running after its input closed before it is killed.
const RUN_AFTER_CLOSE: Duration = Duration::from_secs(300);

/// Starts the helper that the daemon's `event_forwarding` settings name for each command with
/// event forwarding keys, and writes it the command's event log.
pub(crate) struct EventForwarder(Arc<Shared>);

struct Shared {
    launcher: Box<dyn LaunchHelper>,
}

impl EventForwarder {
    pub(crate) fn new(launcher: Box<dyn LaunchHelper>) -> Self {
        Self(Arc::new(Shared { launcher }))
    }
}

impl CommandLifecycleObserver for EventForwarder {
    fn command_started(&self, command: &StartedCommand<'_>) -> Option<Box<dyn CommandObserver>> {
        if command.event_forwarding_keys.is_empty() {
            return None;
        }
        let settings = command.event_forwarding_settings;
        if !settings.enabled() {
            return None;
        }
        let helper = settings.helper().filter(|helper| !helper.is_empty())?;
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let working_dir = command.project_root.root().as_abs_path().to_owned();
        let run = Run {
            program: working_dir.join(helper),
            working_dir,
            keys: command.event_forwarding_keys.join(","),
            header: encode::header(command),
        };
        let (result_tx, result_rx) = oneshot::channel();
        runtime.spawn(forward(self.0.dupe(), run, result_rx));
        Some(Box::new(Forwarding { result: result_tx }))
    }
}

/// What a command's forwarder needs to start its helper.
struct Run {
    program: AbsPathBuf,
    working_dir: AbsPathBuf,
    /// The command's event forwarding keys, separated by commas.
    keys: String,
    /// The event log's header, written first.
    header: Vec<u8>,
}

/// Hands a keyed command's result to its forwarder. Dropping it without a result tells the
/// forwarder that the command stopped without one.
struct Forwarding {
    result: oneshot::Sender<Vec<u8>>,
}

impl CommandObserver for Forwarding {
    fn command_ended(self: Box<Self>, result: &CommandResult) {
        // Fails only when the forwarder already stopped, and then nobody needs the result.
        let _ignored = self.result.send(encode::result(result));
    }
}

/// Starts a run's helper, writes it the command's event log, closes its input, and ends it if it
/// does not exit by itself in time.
async fn forward(shared: Arc<Shared>, run: Run, mut result: oneshot::Receiver<Vec<u8>>) {
    let Some(Helper { input, mut process }) = launch(&shared, &run).await else {
        return;
    };
    let mut input = Input::new(Box::new(encode::compressed(input)));
    input.give(run.header);
    input.start();

    let mut give_up_at = None;
    let exited = loop {
        if give_up_at.is_some() && !input.is_writing() {
            // The result, if the command has one, is written, and the stream ended.
            break false;
        }
        tokio::select! {
            biased;
            () = process.exited() => break true,
            written = input.finish_write() => {
                if written.is_err() {
                    // The helper stopped reading its input.
                    break false;
                }
            }
            ended = &mut result, if give_up_at.is_none() => {
                // An error means the command stopped without a result.
                if let Ok(result) = ended {
                    input.give(result);
                }
                input.finish();
                give_up_at = Some(Instant::now() + WRITE_AFTER_END);
            }
            () = sleep_until(give_up_at) => break false,
        }
    };

    drop(input);
    if !exited {
        let _still_running = tokio::time::timeout(RUN_AFTER_CLOSE, process.exited()).await;
    }
    // Kills the helper if it still runs, and ends whatever it left behind.
    process.kill().await;
}

/// Starts a run's helper on a thread of its own, as starting a process can block, never on a
/// thread that runs commands.
async fn launch(shared: &Arc<Shared>, run: &Run) -> Option<Helper> {
    let launched = {
        let shared = shared.dupe();
        let (program, working_dir, keys) = (
            run.program.clone(),
            run.working_dir.clone(),
            run.keys.clone(),
        );
        tokio::task::spawn_blocking(move || shared.launcher.launch(&program, &working_dir, &keys))
            .await
    };
    match launched {
        Ok(Ok(helper)) => Some(helper),
        Ok(Err(error)) => {
            tracing::warn!(
                "Cannot start the event forwarding helper `{}`: {error}",
                run.program.display()
            );
            None
        }
        Err(error) => {
            tracing::debug!(
                "Starting the event forwarding helper `{}` did not finish: {error}",
                run.program.display()
            );
            None
        }
    }
}

/// Never resolves for `None`.
async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

type HelperInput = Box<dyn AsyncWrite + Send + Unpin>;

/// A helper's input. Messages are written in the order they are given, in batches: those given
/// while a batch is being written go together in the next one. Each batch ends with a flush, so a
/// helper that keeps up can decode each message as soon as it is written.
struct Input {
    /// The input, while nothing is being written.
    idle: Option<HelperInput>,
    /// Writes one batch, then gives the input back.
    writing: Option<BoxFuture<'static, io::Result<HelperInput>>>,
    waiting: VecDeque<Vec<u8>>,
    /// Whether to end the stream once the messages given so far are written.
    finishing: bool,
}

impl Input {
    fn new(input: HelperInput) -> Self {
        Self {
            idle: Some(input),
            writing: None,
            waiting: VecDeque::new(),
            finishing: false,
        }
    }

    /// Queues a message, written once [`Input::start`] or [`Input::finish`] is called.
    fn give(&mut self, message: Vec<u8>) {
        self.waiting.push_back(message);
    }

    /// Starts writing the queued messages, or leaves them for the next batch while one is being
    /// written.
    fn start(&mut self) {
        if self.waiting.is_empty() && !self.finishing {
            return;
        }
        if let Some(input) = self.idle.take() {
            let batch = std::mem::take(&mut self.waiting);
            let finish = std::mem::take(&mut self.finishing);
            self.writing = Some(write(input, batch, finish));
        }
    }

    /// Writes the queued messages, then ends the stream.
    fn finish(&mut self) {
        self.finishing = true;
        self.start();
    }

    /// Whether a batch is being written.
    fn is_writing(&self) -> bool {
        self.writing.is_some()
    }

    /// Finishes writing the batch being written, then starts writing the next one, if any. Never
    /// resolves while nothing is being written. Safe to cancel: the write goes on at the next
    /// call.
    async fn finish_write(&mut self) -> io::Result<()> {
        let Some(writing) = &mut self.writing else {
            return std::future::pending().await;
        };
        let written = writing.await;
        self.writing = None;
        self.idle = Some(written?);
        self.start();
        Ok(())
    }
}

/// Writes `batch`, then flushes the input, or ends the compressed stream when `finish` is set.
fn write(
    mut input: HelperInput,
    batch: VecDeque<Vec<u8>>,
    finish: bool,
) -> BoxFuture<'static, io::Result<HelperInput>> {
    async move {
        for message in batch {
            input.write_all(&message).await?;
        }
        if finish {
            input.shutdown().await?;
        } else {
            input.flush().await?;
        }
        Ok(input)
    }
    .boxed()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::task::Context;
    use std::task::Poll;
    use std::time::SystemTime;

    use async_compression::tokio::bufread::ZstdDecoder;
    use buck2_cli_proto::CommandProgress;
    use buck2_cli_proto::command_progress::Progress;
    use buck2_cli_proto::command_result;
    use buck2_core::fs::project::ProjectRoot;
    use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
    use buck2_fs::paths::abs_path::AbsPath;
    use buck2_settings::EventForwarding;
    use buck2_wrapper_common::invocation_id::TraceId;
    use prost::Message;
    use tokio::io::AsyncReadExt;
    use tokio::io::BufReader;
    use tokio::io::DuplexStream;
    use tokio::sync::mpsc;
    use tokio::sync::watch;

    use super::*;
    use crate::helper::HelperProcess;

    const KEY: &str = "8b6f2c1e-5d4a-4c3b-9a2e-1f0d7c6b5a49";
    const STARTED_UNIX_MS: u64 = 1792164531000;
    /// About the size of an operating system pipe's buffer.
    const ROOMY_PIPE: usize = 64 * 1024;

    /// How a fake helper's launch goes.
    enum Launch {
        /// The helper starts, and its input holds this many bytes that it has not read.
        Pipe(usize),
        /// The helper does not start.
        Fail,
        /// Starting the helper blocks its thread until the test lets it go; then it starts with
        /// a roomy pipe.
        Blocked(std::sync::mpsc::Receiver<()>),
    }

    /// Starts fake helpers: each takes the next scripted launch, and launches past the script
    /// start with a roomy pipe. Hands the test's side of each helper to the test.
    struct FakeLauncher {
        launches: Mutex<VecDeque<Launch>>,
        started: mpsc::UnboundedSender<FakeHelper>,
    }

    impl LaunchHelper for FakeLauncher {
        fn launch(
            &self,
            program: &AbsPath,
            working_dir: &AbsPath,
            keys: &str,
        ) -> io::Result<Helper> {
            let launch = self.launches.lock().unwrap().pop_front();
            let capacity = match launch {
                None => ROOMY_PIPE,
                Some(Launch::Pipe(capacity)) => capacity,
                Some(Launch::Fail) => return Err(io::Error::other("the helper cannot start")),
                Some(Launch::Blocked(release)) => {
                    let _released = release.recv();
                    ROOMY_PIPE
                }
            };
            let (pipe, output) = tokio::io::duplex(capacity);
            let exit = Arc::new(watch::Sender::new(None));
            let writes_after_exit = Arc::new(AtomicUsize::new(0));
            let left_behind_ended = Arc::new(AtomicBool::new(false));
            self.started
                .send(FakeHelper {
                    program: program.to_owned(),
                    working_dir: working_dir.to_owned(),
                    keys: keys.to_owned(),
                    output: Some(ZstdDecoder::new(BufReader::new(output))),
                    exit: exit.dupe(),
                    writes_after_exit: writes_after_exit.dupe(),
                    left_behind_ended: left_behind_ended.dupe(),
                })
                .unwrap();
            Ok(Helper {
                input: Box::new(FakeInput {
                    pipe,
                    exit: exit.subscribe(),
                    writes_after_exit,
                }),
                process: Box::new(FakeProcess {
                    exited: exit.subscribe(),
                    exit,
                    left_behind_ended,
                }),
            })
        }
    }

    /// How a fake helper ended, and when.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Exit {
        Exited(Instant),
        Killed(Instant),
    }

    /// A fake helper's input, which counts the writes made after the helper ended.
    struct FakeInput {
        pipe: DuplexStream,
        exit: watch::Receiver<Option<Exit>>,
        writes_after_exit: Arc<AtomicUsize>,
    }

    impl AsyncWrite for FakeInput {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            if self.exit.borrow().is_some() {
                self.writes_after_exit.fetch_add(1, Ordering::Relaxed);
            }
            Pin::new(&mut self.pipe).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut self.pipe).poll_shutdown(cx)
        }
    }

    struct FakeProcess {
        exit: Arc<watch::Sender<Option<Exit>>>,
        exited: watch::Receiver<Option<Exit>>,
        /// Whether the processes the helper left behind were ended.
        left_behind_ended: Arc<AtomicBool>,
    }

    impl HelperProcess for FakeProcess {
        fn exited(&mut self) -> BoxFuture<'_, ()> {
            async move {
                self.exited
                    .wait_for(Option::is_some)
                    .await
                    .expect("the fake process holds its sender");
            }
            .boxed()
        }

        fn kill(&mut self) -> BoxFuture<'_, ()> {
            self.exit.send_if_modified(|exit| {
                let running = exit.is_none();
                if running {
                    *exit = Some(Exit::Killed(Instant::now()));
                }
                running
            });
            self.left_behind_ended.store(true, Ordering::Relaxed);
            async {}.boxed()
        }
    }

    /// The test's side of a fake helper: what was written to its input, and its exit.
    struct FakeHelper {
        program: AbsPathBuf,
        working_dir: AbsPathBuf,
        keys: String,
        /// What the helper reads, decompressed; `None` once the helper exited.
        output: Option<ZstdDecoder<BufReader<DuplexStream>>>,
        exit: Arc<watch::Sender<Option<Exit>>>,
        writes_after_exit: Arc<AtomicUsize>,
        left_behind_ended: Arc<AtomicBool>,
    }

    impl FakeHelper {
        /// The next length-delimited message the helper reads, or `None` once its input closed.
        async fn message(&mut self) -> Option<Vec<u8>> {
            let output = self.output.as_mut().expect("the helper still runs");
            let read = async {
                let mut delimiter = Vec::new();
                loop {
                    let mut byte = [0];
                    if output.read(&mut byte).await.unwrap() == 0 {
                        assert!(delimiter.is_empty(), "the input closed within a message");
                        return None;
                    }
                    delimiter.push(byte[0]);
                    if byte[0] & 0x80 == 0 {
                        break;
                    }
                }
                let len = prost::decode_length_delimiter(delimiter.as_slice()).unwrap();
                let mut message = vec![0; len];
                output.read_exact(&mut message).await.unwrap();
                Some(message)
            };
            tokio::time::timeout(Duration::from_secs(3600), read)
                .await
                .expect("the helper should get a message or the end of its input")
        }

        /// The event log's header, the first message.
        async fn header(&mut self) -> buck2_data::Invocation {
            let header = self.message().await.expect("the header");
            buck2_data::Invocation::decode(header.as_slice()).unwrap()
        }

        /// The next message after the header, or `None` once the helper's input closed.
        async fn item(&mut self) -> Option<Progress> {
            let item = self.message().await?;
            Some(
                CommandProgress::decode(item.as_slice())
                    .unwrap()
                    .progress
                    .expect("a message holds an event or the result"),
            )
        }

        /// Every message after the header until the helper's input closes.
        async fn items_until_closed(&mut self) -> Vec<Progress> {
            let mut items = Vec::new();
            while let Some(item) = self.item().await {
                items.push(item);
            }
            items
        }

        /// The helper exits by itself, without reading the rest of its input.
        fn exit(&mut self) {
            self.output = None;
            self.exit.send_replace(Some(Exit::Exited(Instant::now())));
        }

        /// Waits until the forwarder is done with the helper.
        async fn released(&self) {
            tokio::time::timeout(Duration::from_secs(3600), self.exit.closed())
                .await
                .expect("the forwarder should let go of the helper");
        }

        /// Waits until the helper exited or was killed.
        async fn ended(&self) -> Exit {
            let mut exit = self.exit.subscribe();
            let exit =
                tokio::time::timeout(Duration::from_secs(3600), exit.wait_for(Option::is_some))
                    .await
                    .expect("the helper should end")
                    .unwrap();
            exit.expect("waited for an exit")
        }
    }

    struct Harness {
        forwarder: EventForwarder,
        launched: mpsc::UnboundedReceiver<FakeHelper>,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(vec![])
        }

        fn with(launches: Vec<Launch>) -> Self {
            let (started, launched) = mpsc::unbounded_channel();
            let launcher = FakeLauncher {
                launches: Mutex::new(VecDeque::from(launches)),
                started,
            };
            Self {
                forwarder: EventForwarder::new(Box::new(launcher)),
                launched,
            }
        }

        /// The next helper started. Fails at once when none starts: the paused clock skips the
        /// hour.
        async fn helper(&mut self) -> FakeHelper {
            tokio::time::timeout(Duration::from_secs(3600), self.launched.recv())
                .await
                .expect("a helper should start")
                .unwrap()
        }

        /// How many helpers have started and not been taken with `helper`.
        fn helpers_started(&mut self) -> usize {
            let mut count = 0;
            while self.launched.try_recv().is_ok() {
                count += 1;
            }
            count
        }
    }

    /// A command, owning what a `StartedCommand` borrows.
    struct TestCommand {
        trace_id: TraceId,
        argv: Vec<String>,
        working_dir: String,
        project_root: ProjectRoot,
        keys: Vec<String>,
        settings: EventForwarding,
    }

    impl TestCommand {
        fn new() -> Self {
            let root = if cfg!(windows) {
                "C:\\repos\\project"
            } else {
                "/data/repos/project"
            };
            Self {
                trace_id: TraceId::new(),
                argv: ["buck2", "build", "//foo:bar"].map(str::to_owned).to_vec(),
                working_dir: root.to_owned(),
                project_root: ProjectRoot::new_unchecked(AbsNormPathBuf::unchecked_new(
                    PathBuf::from(root),
                )),
                keys: vec![KEY.to_owned()],
                settings: settings(r#"{"enabled": true, "helper": "tools/helper"}"#),
            }
        }

        fn start(&self, harness: &Harness) -> Option<Box<dyn CommandObserver>> {
            harness.forwarder.command_started(&StartedCommand {
                trace_id: &self.trace_id,
                sanitized_argv: &self.argv,
                working_dir: &self.working_dir,
                project_root: &self.project_root,
                event_forwarding_keys: &self.keys,
                event_forwarding_settings: &self.settings,
                start_time: start_time(),
            })
        }

        fn helper_path(&self) -> AbsPathBuf {
            self.project_root.root().as_abs_path().join("tools/helper")
        }
    }

    fn start_time() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_millis(STARTED_UNIX_MS)
    }

    fn settings(json: &str) -> EventForwarding {
        serde_json::from_str(json).unwrap()
    }

    fn build_result() -> CommandResult {
        CommandResult {
            result: Some(command_result::Result::BuildResponse(
                buck2_cli_proto::BuildResponse::default(),
            )),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn an_unkeyed_command_starts_no_helper() {
        let mut harness = Harness::new();
        let command = TestCommand {
            keys: Vec::new(),
            ..TestCommand::new()
        };
        assert!(command.start(&harness).is_none());
        tokio::task::yield_now().await;
        assert_eq!(harness.helpers_started(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn event_forwarding_off_starts_no_helper() {
        let mut harness = Harness::new();
        for json in [
            r#"{}"#,
            r#"{"enabled": false, "helper": "tools/helper"}"#,
            r#"{"enabled": true}"#,
            r#"{"enabled": true, "helper": ""}"#,
        ] {
            let command = TestCommand {
                settings: settings(json),
                ..TestCommand::new()
            };
            assert!(command.start(&harness).is_none(), "{json}");
        }
        tokio::task::yield_now().await;
        assert_eq!(harness.helpers_started(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_keyed_command_starts_one_helper_at_its_start() {
        let mut harness = Harness::new();
        let command = TestCommand {
            keys: vec![KEY.to_owned(), "second".to_owned()],
            ..TestCommand::new()
        };
        let _observer = command.start(&harness).unwrap();

        let mut helper = harness.helper().await;
        // The header comes before the command ends.
        let header = helper.header().await;
        assert_eq!(
            (
                helper.program.clone(),
                helper.working_dir.clone(),
                helper.keys.as_str()
            ),
            (
                command.helper_path(),
                command.project_root.root().as_abs_path().to_owned(),
                format!("{KEY},second").as_str()
            )
        );
        assert_eq!(
            header,
            buck2_data::Invocation {
                command_line_args: command.argv.clone(),
                expanded_command_line_args: Vec::new(),
                working_dir: command.working_dir.clone(),
                trace_id: Some(command.trace_id.to_string()),
                start_time: Some(start_time().into()),
            }
        );
        assert_eq!(harness.helpers_started(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn the_helper_gets_the_header_then_the_result_then_its_input_closes() {
        let mut harness = Harness::new();
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        helper.header().await;

        observer.command_ended(&build_result());
        assert_eq!(
            helper.items_until_closed().await,
            vec![Progress::Result(Box::new(build_result()))]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_without_a_result_closes_the_input_after_the_header() {
        let mut harness = Harness::new();
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        helper.header().await;

        drop(observer);
        assert_eq!(helper.items_until_closed().await, vec![]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_helper_never_holds_up_the_command() {
        // The helper never reads: its input takes one byte, then every write waits.
        let mut harness = Harness::with(vec![Launch::Pipe(1)]);
        let observer = TestCommand::new().start(&harness).unwrap();
        let helper = harness.helper().await;

        // The observer's calls return while the forwarder still waits for the helper.
        observer.command_ended(&build_result());
        assert_eq!(*helper.exit.borrow(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_helper_that_is_slow_to_start_holds_up_no_other_command() {
        let (release, released) = std::sync::mpsc::channel();
        let mut harness = Harness::with(vec![Launch::Blocked(released)]);
        let _slow = TestCommand::new().start(&harness).unwrap();
        // Lets the slow start go after real seconds, should it hold up the runtime's only
        // thread, so that the test fails instead of hanging.
        let let_go = Arc::new(AtomicBool::new(false));
        let (other_started, wait_for_other) = std::sync::mpsc::channel::<()>();
        let watchdog = {
            let let_go = let_go.dupe();
            std::thread::spawn(move || {
                let _started = wait_for_other.recv_timeout(Duration::from_secs(10));
                let_go.store(true, Ordering::SeqCst);
                let _ignored = release.send(());
            })
        };

        let _other = TestCommand::new().start(&harness).unwrap();
        let mut other = harness.helper().await;
        other.header().await;
        let slow_start_was_let_go = let_go.load(Ordering::SeqCst);
        other_started.send(()).unwrap();
        watchdog.join().unwrap();
        assert!(
            !slow_start_was_let_go,
            "the other command's helper should start while the slow one is still starting"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_helper_still_gets_the_result() {
        // The command ends while the header waits for the helper to read it.
        let mut harness = Harness::with(vec![Launch::Pipe(1)]);
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        observer.command_ended(&build_result());

        helper.header().await;
        assert_eq!(
            helper.items_until_closed().await,
            vec![Progress::Result(Box::new(build_result()))]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn the_forwarder_gives_up_on_the_result_60_s_after_the_command_ended() {
        let mut harness = Harness::with(vec![Launch::Pipe(1)]);
        let observer = TestCommand::new().start(&harness).unwrap();
        let helper = harness.helper().await;

        let ended_at = Instant::now();
        observer.command_ended(&build_result());
        // It closes the input at 60 s, then kills the helper 300 s later.
        assert_eq!(
            helper.ended().await,
            Exit::Killed(ended_at + WRITE_AFTER_END + RUN_AFTER_CLOSE)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_helper_that_cannot_start_costs_only_that_commands_messages() {
        let mut harness = Harness::with(vec![Launch::Fail]);
        let failed = TestCommand::new().start(&harness).unwrap();
        failed.command_ended(&build_result());

        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        helper.header().await;
        observer.command_ended(&build_result());
        assert_eq!(
            helper.items_until_closed().await,
            vec![Progress::Result(Box::new(build_result()))]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_helper_that_exited_gets_no_more_writes() {
        let mut harness = Harness::new();
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        helper.header().await;
        let exited_at = Instant::now();
        helper.exit();

        observer.command_ended(&build_result());
        helper.released().await;
        assert_eq!(
            (
                helper.writes_after_exit.load(Ordering::Relaxed),
                *helper.exit.borrow()
            ),
            (0, Some(Exit::Exited(exited_at)))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_helper_still_running_past_its_bound_is_killed() {
        let mut harness = Harness::new();
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        observer.command_ended(&build_result());
        helper.header().await;
        helper.items_until_closed().await;
        let closed_at = Instant::now();

        assert_eq!(
            helper.ended().await,
            Exit::Killed(closed_at + RUN_AFTER_CLOSE)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_helper_that_exits_after_its_input_closed_is_not_killed() {
        let mut harness = Harness::new();
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        observer.command_ended(&build_result());
        helper.header().await;
        helper.items_until_closed().await;
        let exited_at = Instant::now();
        helper.exit();

        assert_eq!(helper.ended().await, Exit::Exited(exited_at));
    }

    #[tokio::test(start_paused = true)]
    async fn the_processes_a_helper_leaves_behind_are_ended_when_it_exits_after_its_input_closed() {
        let mut harness = Harness::new();
        let observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        observer.command_ended(&build_result());
        helper.header().await;
        helper.items_until_closed().await;
        helper.exit();

        helper.released().await;
        assert!(helper.left_behind_ended.load(Ordering::Relaxed));
    }

    #[tokio::test(start_paused = true)]
    async fn the_processes_a_helper_leaves_behind_are_ended_when_it_exits_early() {
        let mut harness = Harness::new();
        let _observer = TestCommand::new().start(&harness).unwrap();
        let mut helper = harness.helper().await;
        helper.header().await;
        helper.exit();

        helper.released().await;
        assert!(helper.left_behind_ended.load(Ordering::Relaxed));
    }
}
