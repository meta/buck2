/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::ops::ControlFlow;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_agent_interactive::Decision;
use buck2_cli_proto::CommandResult;
use buck2_cli_proto::command_result;
use buck2_error::BuckErrorContext;
use buck2_error::BuckErrorOptionContext;
use buck2_error::ErrorTag;
use buck2_event_log::stream_value::StreamValue;
use buck2_events::BuckEvent;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_wrapper_common::invocation_id::TraceId;
use futures::Future;
use futures::FutureExt;
use futures::Stream;
use futures::StreamExt;
use futures::stream;
use futures::stream::FuturesUnordered;
use tokio::runtime::Runtime;

use crate::client_cpu_tracker::ClientCpuTracker;
use crate::client_ctx::AgentAdvice;
use crate::command_outcome::CommandOutcome;
use crate::console_interaction_stream::ConsoleInteraction;
use crate::console_interaction_stream::ConsoleInteractionStream;
use crate::console_interaction_stream::NoopSuperConsoleInteraction;
use crate::console_interaction_stream::SuperConsoleInteraction;
use crate::daemon::client::BuckdClient;
use crate::daemon::client::NoPartialResultHandler;
use crate::daemon::client::tonic_status_to_error;
use crate::exit_result::ExitResult;
use crate::file_tailers::tailers::FileTailers;
use crate::subscribers::observer::ErrorObserver;
use crate::subscribers::recorder::InvocationRecorder;
use crate::subscribers::subscriber::EventSubscriber;
use crate::ticker::Tick;
use crate::ticker::Ticker;

/// Target number of self.tick() calls per second. These can be used by implementations for regular updates, for example
/// superconsole uses it to re-render the frame and this is what allows it to have constantly updating timers.
/// Other than tick() calls, implementations will only be notified when new events arrive.
const TICKS_PER_SECOND: u32 = 10;

/// The client message processing loop will try to yield if this much time has
/// passed since the last yield. This guarantees that any outer futures get
/// polled occasionally, notably the Ctrl-C-detecting future, allowing the
/// client loop future to get dropped when the user interrupts us, even if we're
/// processing continuous streams of messages from the server.
const STREAM_PROCESSING_YIELD_INTERVAL: Duration = Duration::from_millis(125);

#[derive(Debug, buck2_error::Error)]
#[allow(clippy::large_enum_variant)]
enum BuckdCommunicationError {
    #[error("call to daemon returned an unexpected result type. got `{0:?}`")]
    #[buck2(tag = Tier0)]
    UnexpectedResultType(command_result::Result),
    #[error("buck daemon returned an empty CommandResult")]
    #[buck2(tag = Tier0)]
    EmptyCommandResult,
    #[error("buck daemon request finished without returning a CommandResult")]
    #[buck2(tag = MissingCommandResult)]
    MissingCommandResult,
    #[error(
        "The Buck2 daemon was shut down while executing your command. This happened because: {0}"
    )]
    #[buck2(tag = InterruptedByDaemonShutdown)]
    InterruptedByDaemonShutdown(buck2_data::DaemonShutdown),
    #[error("buckd communication encountered an unexpected error `{0:?}`")]
    #[buck2(tag = Tier0)]
    TonicError(tonic::Status),
}

impl From<tonic::Status> for BuckdCommunicationError {
    fn from(status: tonic::Status) -> Self {
        match status.code() {
            tonic::Code::Ok => {
                unreachable!("::Ok should be unreachable as it should produce an Ok result")
            }
            // all errors should be encoded into the CommandResult, we must've hit something strange to be here.
            _ => BuckdCommunicationError::TonicError(status),
        }
    }
}

#[async_trait]
pub trait PartialResultHandler {
    type PartialResult: TryFrom<
            buck2_cli_proto::partial_result::PartialResult,
            Error = buck2_cli_proto::partial_result::PartialResult,
        >;

    async fn handle_partial_result(
        &mut self,
        ctx: PartialResultCtx<'_>,
        partial_res: Self::PartialResult,
    ) -> buck2_error::Result<()>;
}

/// Exposes restricted access to EventsCtx from PartialResultHandler instances.
pub struct PartialResultCtx<'a> {
    inner: &'a mut EventsCtx,
}

impl PartialResultCtx<'_> {
    pub async fn stdout(&mut self, bytes: &[u8]) -> buck2_error::Result<()> {
        self.inner
            .try_for_each_subscriber(|subscriber| subscriber.handle_output(bytes))
            .await
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum FileTailerEvent {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
}

/// Manages incoming event streams from the daemon for the buck2 client and
/// forwards them to the appropriate subscribers in EventsCtx
pub struct DaemonEventsCtx<'a> {
    pub(crate) inner: &'a mut EventsCtx,
    tailers: FileTailers,
    ticker: Ticker,
}

impl<'a> DaemonEventsCtx<'a> {
    async fn yield_if_stream_processing_took_too_long(
        last_yield: &mut Instant,
        _location: &'static str,
    ) {
        let elapsed = Instant::now() - *last_yield;
        if elapsed >= STREAM_PROCESSING_YIELD_INTERVAL {
            tokio::task::yield_now().await;
            *last_yield = Instant::now();
        }
    }

    pub(crate) fn new(
        client: &mut BuckdClient,
        events_ctx: &'a mut EventsCtx,
    ) -> buck2_error::Result<Self> {
        let tailers = FileTailers::new(&client.daemon_dir)?;
        Ok(Self {
            inner: events_ctx,
            tailers,
            ticker: Ticker::new(TICKS_PER_SECOND),
        })
    }

    pub fn without_tailers(events_ctx: &'a mut EventsCtx) -> Self {
        Self {
            inner: events_ctx,
            tailers: FileTailers::empty(),
            ticker: Ticker::new(TICKS_PER_SECOND),
        }
    }

    async fn handle_stream_next<Handler>(
        &mut self,
        partial_result_handler: &mut Handler,
        next: Option<Vec<buck2_error::Result<StreamValue>>>,
        shutdown: &mut Option<buck2_data::DaemonShutdown>,
        last_yield: &mut Instant,
    ) -> buck2_error::Result<ControlFlow<Box<CommandResult>, ()>>
    where
        Handler: PartialResultHandler,
    {
        let next = next.ok_or(BuckdCommunicationError::MissingCommandResult)?;
        let mut events = Vec::with_capacity(next.len());
        for next in next {
            let next = match next {
                Ok(next) => next,
                Err(e) => {
                    self.inner.handle_events(events, shutdown).await?;
                    let oom_reason = self.inner.daemon_oom_reason().await?;
                    return if let Some(oom_reason) = oom_reason {
                        Err(e)
                            .buck_error_context(
                                format!("Buck2 daemon was killed by an OOM killer due to high memory pressure ({oom_reason}). \
                                Common causes are large or numerous build or test targets or \
                                too many Buck2 daemons running simultaneously."))
                            .tag(ErrorTag::ClientGrpcStream)
                            .tag(ErrorTag::DaemonOomKilled)
                    } else {
                        Err(e)
                            .buck_error_context("Buck daemon event bus encountered an error, the root cause (if available) is displayed above this message.")
                            .tag(ErrorTag::ClientGrpcStream)
                    };
                }
            };
            match next {
                StreamValue::Event(event) => {
                    let event = event.try_into()?;
                    events.push(event);
                }
                StreamValue::PartialResult(partial_res) => {
                    let partial_res = partial_res
                        .partial_result
                        .internal_error("Empty partial result")?
                        .try_into()
                        .map_err(|e| {
                            buck2_error::buck2_error!(
                                buck2_error::ErrorTag::Tier0,
                                "Invalid PartialResult: {:?}",
                                e
                            )
                        })?;
                    partial_result_handler
                        .handle_partial_result(PartialResultCtx { inner: self.inner }, partial_res)
                        .await?;
                }
                StreamValue::Result(res) => {
                    self.inner.handle_events(events, shutdown).await?;
                    self.inner.handle_command_result(&res).await?;
                    return Ok(ControlFlow::Break(res));
                }
            }

            Self::yield_if_stream_processing_took_too_long(last_yield, "handle_stream_next").await;
        }
        self.inner.handle_events(events, shutdown).await?;
        Ok(ControlFlow::Continue(()))
    }

    async fn dispatch_tailer_event(&mut self, event: FileTailerEvent) -> buck2_error::Result<()> {
        match event {
            FileTailerEvent::Stdout(out) | FileTailerEvent::Stderr(out) => {
                // Sending daemon stdout to stderr.
                // Daemon is not supposed to write anything to stdout.
                // But if daemon does, it should not be used as standard output of buck2.
                self.inner.handle_tailer_stderr(&out).await
            }
        }
    }

    async fn unpack_stream_inner<S, Handler>(
        &mut self,
        partial_result_handler: &mut Handler,
        stream: S,
        mut console_interaction: Option<ConsoleInteractionStream<'_>>,
    ) -> buck2_error::Result<CommandResult>
    where
        S: Stream<Item = buck2_error::Result<StreamValue>>,
        Handler: PartialResultHandler,
    {
        let mut noop_console_interaction = NoopSuperConsoleInteraction;
        let console_interaction: &mut dyn SuperConsoleInteraction = match &mut console_interaction {
            Some(i) => i as _,
            None => &mut noop_console_interaction as _,
        };

        let stream = stream.ready_chunks(1000);
        let mut stream = pin!(stream);

        // NOTE: When unpacking the stream we capture any shutdown event we encounter. If we fail
        // to unpack the stream to completion, we'll use that later.
        let mut shutdown = None;
        let mut last_yield = Instant::now();

        let command_result: buck2_error::Result<CommandResult> = try {
            loop {
                tokio::select! {
                    next = stream.next() => {
                        // Make sure we still flush if next produces an error is accurate
                        match self.handle_stream_next(
                            partial_result_handler,
                            next,
                            &mut shutdown,
                            &mut last_yield,
                        ).await? {
                            ControlFlow::Continue(()) => {}
                            ControlFlow::Break(res) => break *res,
                        }
                    }
                    Some(event) = self.tailers.recv() => {
                        self.dispatch_tailer_event(event).await?;
                    }
                    interaction = console_interaction.interaction() => {
                        self.inner.handle_console_interaction(&interaction?).await?;
                    }
                    tick = self.ticker.tick() => {
                        self.inner.tick(&tick).await?;
                        if let Some(tps) = self.inner.desired_ticks_per_second() {
                            self.ticker.set_ticks_per_second(tps);
                        } else {
                            self.ticker.set_ticks_per_second(TICKS_PER_SECOND);
                        }
                    }
                }
            }
        };

        let flush_result = self.flush().await;
        self.inner.handle_stream_end();

        let command_result = match (command_result, shutdown) {
            (Ok(r), _) => r,
            (Err(e), Some(shutdown)) => {
                // NOTE: When we get an error unpacking the stream, that means we didn't get a
                // CommandResult, or we had an error on the stream. Both of those things are *most
                // often* caused by a daemon shutdown, so if we did have a daemon shutdown reported
                // to us over the event stream, we show that instead. We log the real error for
                // debugging in case that's useful, but it's sufficiently likely (as in: almost
                // certain) the daemon shutdown is the cause for us to simply claim it is.
                tracing::debug!("Original unpack_stream error was: {:#}", e);

                return Err(BuckdCommunicationError::InterruptedByDaemonShutdown(shutdown).into());
            }
            (Err(e), None) => return Err(e),
        };

        flush_result?;
        Ok(command_result)
    }

    /// Given a stream of StreamValues originating from the daemon, "unpacks" it by extracting the command result from the
    /// event stream and returning it.
    /// Also merges all other streams into a single, larger stream
    pub async fn unpack_stream<S, Res, Handler>(
        &mut self,
        partial_result_handler: &mut Handler,
        stream: S,
        console_interaction: Option<ConsoleInteractionStream<'_>>,
    ) -> buck2_error::Result<CommandOutcome<Res>>
    where
        S: Stream<Item = buck2_error::Result<StreamValue>>,
        Res: TryFrom<command_result::Result, Error = command_result::Result>,
        Handler: PartialResultHandler,
    {
        let command_result = self
            .unpack_stream_inner(partial_result_handler, stream, console_interaction)
            .await;

        let result = self.flush().await.and(command_result);

        match result {
            Ok(result) => convert_result(result),
            Err(err) => Err(self.handle_error_owned(err).await),
        }
    }

    /// Unpack a single `CommandResult`, log any failures if necessary, and convert it to a
    /// `CommandOutcome`
    pub async fn unpack_oneshot<
        Res: TryFrom<command_result::Result, Error = command_result::Result>,
        Fut: Future<Output = Result<tonic::Response<CommandResult>, tonic::Status>>,
    >(
        &mut self,
        f: Fut,
    ) -> buck2_error::Result<CommandOutcome<Res>> {
        let stream = stream::once(f.map(|result| {
            result
                .map(|command_result| StreamValue::Result(Box::new(command_result.into_inner())))
                .map_err(tonic_status_to_error)
        }));
        self.unpack_stream(&mut NoPartialResultHandler, stream, None)
            .await
    }

    async fn handle_error_owned(&mut self, error: buck2_error::Error) -> buck2_error::Error {
        let result: Result<(), buck2_error::Error> = self
            .inner
            .try_for_each_subscriber(|subscriber| subscriber.handle_error(&error))
            .await;
        match result {
            Ok(()) => error,
            Err(e) => EventsCtxError::WrappedStreamError {
                source: error,
                other: e,
            }
            .into(),
        }
    }

    pub async fn flush(&mut self) -> buck2_error::Result<()> {
        let Some(mut stream) = self.tailers.stop_reading() else {
            return Ok(());
        };

        // We need to loop again to drain stdout/stderr
        let mut complete = false;
        while !complete {
            tokio::select! {
                event = stream.recv() => {
                    match event {
                        Some(event) => {self.dispatch_tailer_event(event).await?;}
                        None => {complete = true;}
                    }
                }
                tick = self.ticker.tick() => {
                    self.inner.tick(&tick).await?;
                    if let Some(tps) = self.inner.desired_ticks_per_second() {
                        self.ticker.set_ticks_per_second(tps);
                    } else {
                        self.ticker.set_ticks_per_second(TICKS_PER_SECOND);
                    }
                }
            }
        }

        let tick = self.ticker.tick_now();
        self.inner.tick(&tick).await?;

        Ok(())
    }
}

// TODO(ctolliday) disabled because quiet: true appears broken for client side soft_errors.
// This also fires for cancelled commands, we should check for that if enabling this, if possible.
// Can't flush in drop because it needs to be async, but we can report a soft error if we didn't flush.
// impl<'a> Drop for DaemonEventsCtx<'a> {
//     fn drop(&mut self) {
//         if self.tailers.stream.is_some() {
//             soft_error!(
//                 "daemon_tailers_not_flushed",
//                 internal_error!("DaemonEventsCtx should have been flushed before being dropped"),
//             );
//         }
//     }
// }

/// Convert a CommandResult into a CommandOutcome after the CommandResult has been printed by `handle_command_result`.
fn convert_result<R: TryFrom<command_result::Result, Error = command_result::Result>>(
    value: CommandResult,
) -> buck2_error::Result<CommandOutcome<R>> {
    match value.result {
        Some(command_result::Result::Error(error)) => Ok(CommandOutcome::Failure(
            ExitResult::from_command_result_errors(vec![error]),
        )),
        Some(value) => match value.try_into() {
            Ok(v) => Ok(CommandOutcome::Success(v)),
            Err(res) => Err(BuckdCommunicationError::UnexpectedResultType(res).into()),
        },
        None => Err(BuckdCommunicationError::EmptyCommandResult.into()),
    }
}

/// Forwards events to a list of subscribers to record and report build events.
pub struct EventsCtx {
    pub recorder: Option<Box<InvocationRecorder>>,
    pub(crate) subscribers: Vec<Box<dyn EventSubscriber>>,
    client_cpu_tracker: ClientCpuTracker,
    // buck_log_dir and command_report_path are used to write the command report.
    // Ensuring a command report is always written would require either simplifying
    // how the isolation dir is determined, or writing to a different path.
    pub buck_log_dir: Option<AbsNormPathBuf>,
    pub command_report_path: Option<AbsPathBuf>,
    // Internal commands triggered by other commands should not log an invocation record.
    pub log_invocation_record: bool,
    pub daemon_pid: Option<i64>,
    // The daemon process's cgroup path as read from /proc/{pid}/cgroup by the client. Unlike the
    // `daemon_cgroup_path` the daemon reports about itself, this is available even when the daemon
    // isn't running with resource control.
    pub cgroup_path_of_buck2_daemon: Option<String>,
    pub daemon_start_instant: Option<Instant>,
    /// Whether a superconsole was actually constructed for this command.
    /// Set by `streaming.rs` from the authoritative answer returned by
    /// `get_console_with_root`. Defaults to `false` for non-streaming entry
    /// points (e.g. log replay) that don't go through that path.
    pub used_superconsole: bool,
}

impl EventsCtx {
    pub fn new(
        recorder: Option<InvocationRecorder>,
        subscribers: Vec<Box<dyn EventSubscriber>>,
    ) -> Self {
        Self {
            subscribers,
            recorder: recorder.map(Box::new),
            client_cpu_tracker: ClientCpuTracker::new(),
            buck_log_dir: None,
            command_report_path: None,
            log_invocation_record: true,
            daemon_pid: None,
            cgroup_path_of_buck2_daemon: None,
            daemon_start_instant: None,
            used_superconsole: false,
        }
    }

    /// Describes why the daemon is believed to have been OOM-killed, if it was.
    ///
    /// The evidence is read from `dmesg`, so this never reports anything off Linux.
    #[cfg(target_os = "linux")]
    pub(crate) async fn daemon_oom_reason(&self) -> buck2_error::Result<Option<String>> {
        let Some(pid) = self.daemon_pid else {
            return Ok(None);
        };
        let evidence = crate::subscribers::oom::find_daemon_oom_evidence(
            pid,
            self.cgroup_path_of_buck2_daemon.as_deref(),
            self.daemon_start_instant,
        )
        .await?;
        Ok(evidence.map(|evidence| evidence.to_string()))
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) async fn daemon_oom_reason(&self) -> buck2_error::Result<Option<String>> {
        Ok(None)
    }

    async fn handle_tailer_stderr(&mut self, stderr: &[u8]) -> buck2_error::Result<()> {
        let stderr = String::from_utf8_lossy(stderr);
        let stderr = stderr.trim_end();
        self.try_for_each_subscriber(|subscriber| subscriber.handle_tailer_stderr(stderr))
            .await
    }

    async fn handle_console_interaction(
        &mut self,
        interaction: &ConsoleInteraction,
    ) -> buck2_error::Result<()> {
        self.try_for_each_subscriber(|subscriber| {
            subscriber.handle_console_interaction(interaction)
        })
        .await
    }

    /// Records advice through the invocation's subscribers and rejects blocked operations.
    pub async fn handle_agent_advice(
        &mut self,
        trace_id: TraceId,
        advice: AgentAdvice,
    ) -> buck2_error::Result<()> {
        let tags = advice.tags();
        if !tags.is_empty() {
            self.instant_event(trace_id, buck2_data::TagEvent { tags })
                .await?;
        }
        match advice.decision() {
            Decision::Inactive | Decision::Acknowledged(_) => Ok(()),
            Decision::Block(advice) => Err(buck2_error::buck2_error!(
                ErrorTag::AgentAdviceBlocked,
                "Buck advice [{}]\n{}",
                advice.id(),
                advice.message(),
            )),
        }
    }

    /// Records and displays a client event through the same subscribers as daemon events.
    pub async fn instant_event(
        &mut self,
        trace_id: TraceId,
        data: impl Into<buck2_data::instant_event::Data>,
    ) -> buck2_error::Result<()> {
        let instant = buck2_data::InstantEvent {
            data: Some(data.into()),
        };
        let event = BuckEvent::new(SystemTime::now(), trace_id, None, None, instant.into());
        self.handle_events(vec![event], &mut None).await
    }

    async fn handle_events(
        &mut self,
        events: Vec<BuckEvent>,
        shutdown: &mut Option<buck2_data::DaemonShutdown>,
    ) -> buck2_error::Result<()> {
        let events = events
            .into_iter()
            .map(|mut event| {
                let timestamp = event.timestamp();
                if let buck2_data::buck_event::Data::Instant(instant_event) = event.data_mut() {
                    match &mut instant_event.data {
                        Some(buck2_data::instant_event::Data::Snapshot(snapshot)) => {
                            let now = SystemTime::now();
                            // `None` on overflow.
                            let this_event_client_delay_ms = match now.duration_since(timestamp) {
                                Ok(duration) => i64::try_from(duration.as_millis()).ok(),
                                Err(e) => i64::try_from(e.duration().as_millis())
                                    .ok()
                                    .and_then(|x| x.checked_neg()),
                            };
                            snapshot.this_event_client_delay_ms = this_event_client_delay_ms;

                            snapshot.client_cpu_percents =
                                self.client_cpu_tracker.tick_cpu_time_percents();
                        }
                        Some(buck2_data::instant_event::Data::DaemonShutdown(msg)) => {
                            *shutdown = Some(msg.clone());
                        }
                        _ => {}
                    };
                }
                Arc::new(event)
            })
            .collect::<Vec<_>>();
        self.try_for_each_subscriber(|subscriber| subscriber.handle_events(&events))
            .await
    }

    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        self.try_for_each_subscriber(|subscriber| subscriber.handle_command_result(result))
            .await
    }

    /// This function is called once per `TICK_SPEED`.
    /// A subscriber will have the opportunity to do an arbitrary process at a reliable interval.
    /// In particular, this is crucial for superconsole so that it can draw itself consistently.
    async fn tick(&mut self, tick: &Tick) -> buck2_error::Result<()> {
        self.try_for_each_subscriber(|subscriber| subscriber.tick(tick))
            .await
    }

    /// Return the highest desired tick rate from any subscriber, or `None`
    /// if no subscriber has an opinion.
    fn desired_ticks_per_second(&self) -> Option<u32> {
        let mut result: Option<u32> = None;
        if let Some(recorder) = &self.recorder {
            if let Some(tps) = recorder.desired_ticks_per_second() {
                result = Some(result.map_or(tps, |r: u32| r.max(tps)));
            }
        }
        for s in &self.subscribers {
            if let Some(tps) = s.desired_ticks_per_second() {
                result = Some(result.map_or(tps, |r: u32| r.max(tps)));
            }
        }
        result
    }

    /// Ask subscribers to remove their interactive display from the terminal; used
    /// when the display is being replaced rather than concluded (e.g. `buck2 log
    /// snoop` switching to another invocation).
    pub async fn erase_interactive_output(&mut self) -> buck2_error::Result<()> {
        self.try_for_each_subscriber(|s| s.erase_interactive_output())
            .await
    }

    /// Helper method to abstract the process of applying an `EventSubscriber` method to all of the subscribers.
    /// Quits on the first error encountered.
    pub(crate) async fn try_for_each_subscriber<'b, Fut>(
        &'b mut self,
        mut f: impl FnMut(&'b mut dyn EventSubscriber) -> Fut,
    ) -> buck2_error::Result<()>
    where
        Fut: Future<Output = buck2_error::Result<()>> + 'b,
    {
        if let Some(recorder) = self.recorder.as_mut() {
            f(recorder.as_mut()).await?;
        }

        let mut futures: FuturesUnordered<_> =
            self.subscribers.iter_mut().map(|s| f(s.as_mut())).collect();
        while let Some(res) = futures.next().await {
            res?;
        }
        Ok(())
    }

    /// Helper method to abstract the process of applying an `EventSubscriber` method to all of the subscribers.
    pub(crate) fn for_each_subscriber<'b>(
        &'b mut self,
        mut f: impl FnMut(&'b mut dyn EventSubscriber),
    ) {
        if let Some(recorder) = self.recorder.as_mut() {
            f(recorder.as_mut());
        }

        for subscriber in &mut self.subscribers {
            f(subscriber.as_mut());
        }
    }

    pub(crate) fn handle_stream_end(&mut self) {
        self.for_each_subscriber(|subscriber| subscriber.handle_stream_end());
    }

    pub(crate) fn handle_daemon_connection_failure(&mut self) {
        self.for_each_subscriber(|subscriber| subscriber.handle_daemon_connection_failure());
    }

    pub(crate) fn handle_daemon_started(&mut self, reason: buck2_data::DaemonWasStartedReason) {
        self.for_each_subscriber(|subscriber| subscriber.handle_daemon_started(reason));
    }

    pub(crate) fn handle_should_restart(&mut self) {
        self.for_each_subscriber(|subscriber| subscriber.handle_should_restart());
    }

    pub fn handle_exit_result(&mut self, exit_result: &ExitResult) {
        self.for_each_subscriber(|subscriber| subscriber.handle_exit_result(exit_result));
    }

    pub(crate) fn error_observers(&self) -> impl Iterator<Item = &dyn ErrorObserver> {
        self.recorder.iter().filter_map(|r| r.as_error_observer())
    }

    pub(crate) async fn eprintln(&mut self, message: &str) -> buck2_error::Result<()> {
        self.try_for_each_subscriber(|s| {
            // TODO(nga): this is not a tailer.
            s.handle_tailer_stderr(message)
        })
        .await
    }

    pub async fn finalize(mut self) -> Vec<String> {
        let mut errors = self.finalize_subscribers().await;
        errors.extend(self.finalize_recorder().await);
        errors
    }

    async fn finalize_one(subscriber: Box<dyn EventSubscriber>, errors: &mut Vec<String>) {
        let start = Instant::now();
        let name = subscriber.name().to_owned();
        let res = subscriber.finalize().await;
        let elapsed = Instant::now() - start;
        tracing::info!("Finalizing \'{}\' took {:?}", name, elapsed);

        if let Err(e) = res {
            errors.push(format!(
                "{:?}",
                e.context(format!("\'{}\' failed to finalize", name))
            ));
        }
    }

    /// Finalizes every subscriber except the invocation recorder, in order.
    async fn finalize_subscribers(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        for subscriber in std::mem::take(&mut self.subscribers) {
            Self::finalize_one(subscriber, &mut errors).await;
        }
        errors
    }

    /// Sends the invocation record, if this command logs one.
    async fn finalize_recorder(self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.log_invocation_record
            && let Some(recorder) = self.recorder
        {
            Self::finalize_one(recorder, &mut errors).await;
        }
        errors
    }

    pub fn finalize_events(
        mut self,
        trace_id: TraceId,
        result: ExitResult,
        runtime: &Runtime,
    ) -> ExitResult {
        runtime.block_on(async move {
            let buck_log_dir = self.buck_log_dir.take();
            let command_report_path = self.command_report_path.take();
            let finalize_subscribers = async {
                self.handle_exit_result(&result);
                self.finalize_subscribers().await
            };

            let logging_timeout = Duration::from_secs(30);
            let mut finalizing_errors = tokio::time::timeout(logging_timeout, finalize_subscribers)
                .await
                .unwrap_or_else(|_| {
                    vec![format!(
                        "Timeout after {:?} waiting for logging cleanup",
                        logging_timeout
                    )]
                });

            // The invocation record must not be starved by a slow log upload, so it gets its
            // own budget rather than whatever the other subscribers left of theirs.
            match tokio::time::timeout(logging_timeout, self.finalize_recorder()).await {
                Ok(errors) => finalizing_errors.extend(errors),
                Err(_) => finalizing_errors.push(format!(
                    "Timeout after {:?} waiting for the invocation record",
                    logging_timeout
                )),
            }

            // Don't fail the command if command report fails to write. TODO(ctolliday) show a warning?
            let _unused = result.write_command_report(
                trace_id,
                buck_log_dir,
                command_report_path,
                finalizing_errors,
            );
            result
        })
    }
}

#[derive(buck2_error::Error, Debug)]
#[buck2(tag = Tier0)]
pub enum EventsCtxError {
    #[error("While propagating error:\n{source:#?}, another error was detected:\n{other:#?}")]
    WrappedStreamError {
        #[source]
        source: buck2_error::Error,
        other: buck2_error::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::SystemTime;

    use async_trait::async_trait;
    use buck2_fs::paths::abs_path::AbsPathBuf;
    use buck2_wrapper_common::invocation_id::TraceId;

    use crate::events_ctx::EventsCtx;
    use crate::exit_result::ExitResult;
    use crate::subscribers::recorder::InvocationRecorder;
    use crate::subscribers::subscriber::EventSubscriber;

    struct SlowFinalize(Duration);

    #[async_trait]
    impl EventSubscriber for SlowFinalize {
        fn name(&self) -> &'static str {
            "slow"
        }

        async fn finalize(self: Box<Self>) -> buck2_error::Result<()> {
            tokio::time::sleep(self.0).await;
            Ok(())
        }
    }

    /// The finalizing errors written to the command report when the only other subscriber
    /// takes `slow` to finalize. The recorder has no Scribe sink here, so its `finalize`
    /// leaves a "Scribe sink not enabled" error whenever it runs: that error is the evidence
    /// that the invocation record was attempted.
    fn finalizing_errors(slow: Duration) -> Vec<String> {
        buck2_events::sink::remote::disable();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        let recorder = InvocationRecorder::new(TraceId::new(), None, SystemTime::now(), vec![]);
        let mut ctx = EventsCtx::new(Some(recorder), vec![Box::new(SlowFinalize(slow))]);
        let dir = tempfile::tempdir().unwrap();
        let path = AbsPathBuf::new(dir.path().join("command_report.json")).unwrap();
        ctx.command_report_path = Some(path.clone());
        let res = ctx.finalize_events(TraceId::new(), ExitResult::success(), &runtime);
        assert!(res.is_success());
        let report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        report["finalizing_error_messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn test_recorder_is_finalized_after_a_fast_subscriber() {
        let errors = finalizing_errors(Duration::ZERO);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(
            errors[0].contains("'invocation recorder' failed to finalize"),
            "{errors:?}"
        );
        assert!(errors[0].contains("Scribe sink not enabled"), "{errors:?}");
    }

    /// A subscriber that takes longer than the cleanup budget is reported as a timeout, and the
    /// invocation record is still attempted afterwards.
    #[test]
    fn test_slow_subscriber_does_not_skip_the_invocation_record() {
        let errors = finalizing_errors(Duration::from_secs(31));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert_eq!(errors[0], "Timeout after 30s waiting for logging cleanup");
        assert!(
            errors[1].contains("'invocation recorder' failed to finalize"),
            "{errors:?}"
        );
    }
}
