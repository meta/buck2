/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! The event log of a command, written by the daemon for a client that asks it to and then
//! writes none itself.
//!
//! It holds every event of the command, which a log the client writes does not when the
//! daemon sends the client only some of them. It is where the client's would be.

use std::mem;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_cli_proto::ClientContext;
use buck2_cli_proto::DaemonEventLogRequest;
use buck2_common::argv::Argv;
use buck2_common::argv::ExpandedArgv;
use buck2_common::invocation_paths::TenantPaths;
use buck2_core::error::SoftErrorContext;
use buck2_core::error::with_soft_error_context;
use buck2_core::soft_error;
use buck2_error::BuckErrorOptionContext;
use buck2_event_log::write::EventLogFiles;
use buck2_event_log::write::WriteEventLog;
use buck2_events::Event;
use buck2_events::EventSink;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::forward_rel_path::ForwardRelativePath;
use buck2_fs::working_dir::AbsWorkingDir;
use buck2_settings::InvocationBuckSettings;
use buck2_wrapper_common::invocation_id::TraceId;
use dupe::Dupe;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;

/// How often the events written so far are flushed to the log, as the client does for its own.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Gives a command's events to the task that writes them to the daemon's event log.
pub(crate) struct DaemonEventLogSender {
    events: UnboundedSender<Event>,
}

/// The events of a command that are to be written to the daemon's event log.
pub(crate) struct DaemonEventLogEvents {
    events: UnboundedReceiver<Event>,
    /// When the queue was made, which is before the command's first event. It is the start
    /// time in the log's header.
    started: SystemTime,
    /// The soft error policy of the command. What goes wrong with the log is reported under
    /// it, so that the report goes to the command and counts towards the command's limit:
    /// the task that writes the log is not one of the command's.
    soft_errors: Arc<SoftErrorContext>,
}

pub(crate) fn daemon_event_log(
    soft_errors: Arc<SoftErrorContext>,
) -> (DaemonEventLogSender, DaemonEventLogEvents) {
    let (send, recv) = tokio::sync::mpsc::unbounded_channel();
    (
        DaemonEventLogSender { events: send },
        DaemonEventLogEvents {
            events: recv,
            started: SystemTime::now(),
            soft_errors,
        },
    )
}

impl DaemonEventLogSender {
    /// A sink that sends every event to `sink`, and queues a copy of those that an event log
    /// holds.
    pub(crate) fn tee(self, sink: Arc<dyn EventSink>) -> DaemonEventLogSink {
        DaemonEventLogSink { sender: self, sink }
    }

    /// Queues a copy of `event`, if it is one that an event log holds.
    fn send(&self, event: &Event) {
        // The log ends at the command's result, and events can follow it.
        if matches!(event, Event::PartialResult(..)) || self.events.is_closed() {
            return;
        }
        let _ignored = self.events.send(event.clone());
    }
}

pub(crate) struct DaemonEventLogSink {
    sender: DaemonEventLogSender,
    sink: Arc<dyn EventSink>,
}

#[async_trait]
impl EventSink for DaemonEventLogSink {
    fn send(&self, event: Event) {
        self.sender.send(&event);
        self.sink.send(event);
    }

    async fn send_now(&self, event: Event) {
        self.sender.send(&event);
        self.sink.send_now(event).await;
    }
}

impl DaemonEventLogEvents {
    /// Opens the command's event log under the tenant's output directory and starts writing
    /// the events to it. Returns the event that tells the client where the log is, or `None`
    /// if the log cannot be opened: the command then runs without an event log.
    pub(crate) async fn write(
        self,
        client_ctx: &ClientContext,
        request: &DaemonEventLogRequest,
        paths: &TenantPaths,
    ) -> Option<buck2_data::DaemonEventLog> {
        let soft_errors = self.soft_errors.dupe();
        match self.open(client_ctx, request, paths).await {
            Ok(event_log) => Some(event_log),
            Err(e) => {
                report_incomplete(&soft_errors, e);
                None
            }
        }
    }

    async fn open(
        self,
        client_ctx: &ClientContext,
        request: &DaemonEventLogRequest,
        paths: &TenantPaths,
    ) -> buck2_error::Result<buck2_data::DaemonEventLog> {
        let trace_id: TraceId = client_ctx.trace_id.parse()?;
        let start_time = self.started;
        // A command that sends no settings has their defaults.
        let settings =
            InvocationBuckSettings::from_request(client_ctx.invocation_settings.as_deref())?;
        let files = EventLogFiles::Command {
            logdir: paths
                .buck_out_path()
                .join(ForwardRelativePath::unchecked_new("log")),
            command_name: client_ctx.command_name.clone(),
            retained_event_logs: settings.event_log.retained_logs(),
        };
        let mut log = writer(files, client_ctx, request, start_time)?;
        log.open(trace_id, start_time).await?;
        let path = log
            .command_log()
            .internal_error("The command's event log is open")?
            .display()
            .to_string();
        tokio::spawn(write_events(log, self));

        Ok(buck2_data::DaemonEventLog { path })
    }

    /// Waits for what is queued and appends it to `batch`. Returns false once no more can be
    /// queued.
    async fn recv(&mut self, batch: &mut Vec<Event>) -> bool {
        self.events.recv_many(batch, 1000).await != 0
    }
}

/// A writer of `files` for the command of `client_ctx`. The client's time of starting the
/// command does not reach the daemon: the header has `start_time`.
fn writer(
    files: EventLogFiles,
    client_ctx: &ClientContext,
    request: &DaemonEventLogRequest,
    start_time: SystemTime,
) -> buck2_error::Result<WriteEventLog> {
    let expanded_argv = request.expanded_command_line_args.clone();
    Ok(WriteEventLog::new(
        files,
        AbsWorkingDir::unchecked_new(AbsNormPathBuf::try_from(client_ctx.working_dir.clone())?),
        Argv {
            argv: request.command_line_args.clone(),
            expanded_argv: ExpandedArgv::from_literals(expanded_argv),
        }
        .no_need_to_sanitize(),
        start_time,
        None,
    ))
}

/// Writes `batch` to `log`. Returns false once it holds the command's result.
async fn write_batch(log: &mut WriteEventLog, batch: Vec<Event>) -> buck2_error::Result<bool> {
    let mut buck_events = Vec::with_capacity(batch.len());
    for event in batch {
        match event {
            Event::Buck(event) => buck_events.push(Arc::new(event)),
            Event::PartialResult(..) => {}
            Event::CommandResult(result) => {
                log.write_events(&buck_events).await?;
                log.write_result(&result).await?;
                return Ok(false);
            }
        }
    }
    log.write_events(&buck_events).await?;
    Ok(true)
}

/// Writes events to `log` up to the command's result, or until no more can be queued.
async fn write_events(mut log: WriteEventLog, mut events: DaemonEventLogEvents) {
    let res: buck2_error::Result<()> = try {
        let mut flush = tokio::time::interval(FLUSH_INTERVAL);
        let mut batch = Vec::new();
        loop {
            tokio::select! {
                more = events.recv(&mut batch) => {
                    if !more || !write_batch(&mut log, mem::take(&mut batch)).await? {
                        break;
                    }
                }
                _ = flush.tick() => log.flush_files().await?,
            }
        }
    };
    if let Err(e) = res {
        report_incomplete(&events.soft_errors, e);
    }
    log.exit().await;
}

/// Reports to a command that the daemon's event log of it lacks events, or does not exist.
fn report_incomplete(soft_errors: &Arc<SoftErrorContext>, e: buck2_error::Error) {
    with_soft_error_context(Some(soft_errors.dupe()), || {
        let _unused: buck2_error::Result<()> = try {
            soft_error!("daemon_event_log_incomplete", e);
        };
    })
}

#[cfg(test)]
mod tests {
    use buck2_events::BuckEvent;
    use buck2_events::source::ChannelEventSource;

    use super::*;

    fn event(message: &str) -> Event {
        let message = buck2_data::ConsoleMessage {
            message: message.to_owned(),
        };
        Event::Buck(BuckEvent::new(
            SystemTime::now(),
            TraceId::null(),
            None,
            None,
            buck2_data::InstantEvent {
                data: Some(message.into()),
            }
            .into(),
        ))
    }

    fn message(event: &Event) -> &str {
        use buck2_data::buck_event::Data;
        use buck2_data::instant_event;

        match event {
            Event::Buck(event) => match event.data() {
                Data::Instant(buck2_data::InstantEvent {
                    data: Some(instant_event::Data::ConsoleMessage(message)),
                }) => &message.message,
                data => panic!("Unexpected event: {data:?}"),
            },
            Event::CommandResult(..) => "result",
            Event::PartialResult(..) => "partial result",
        }
    }

    /// The sink of a queue, the queue, and what the sink also sends every event to.
    fn sink() -> (DaemonEventLogSink, DaemonEventLogEvents, ChannelEventSource) {
        let soft_errors = Arc::new(SoftErrorContext::new("", "").unwrap());
        let (sender, events) = daemon_event_log(soft_errors);
        let (source, client) = buck2_events::create_source_sink_pair();
        (sender.tee(Arc::new(client)), events, source)
    }

    async fn queued(events: &mut DaemonEventLogEvents) -> Vec<String> {
        let mut received = Vec::new();
        while !events.events.is_empty() {
            assert!(events.recv(&mut received).await);
        }
        received.iter().map(|e| message(e).to_owned()).collect()
    }

    fn sent_on(source: &mut ChannelEventSource) -> Vec<String> {
        std::iter::from_fn(|| source.try_receive())
            .map(|e| message(&e).to_owned())
            .collect()
    }

    #[tokio::test]
    async fn test_queue_holds_what_an_event_log_holds() {
        let (sink, mut events, mut client) = sink();
        sink.send(event("a"));
        sink.send(Event::PartialResult(Default::default()));
        sink.send(Event::CommandResult(Box::default()));

        assert_eq!(queued(&mut events).await, ["a", "result"]);
        assert_eq!(sent_on(&mut client), ["a", "partial result", "result"]);
    }
}
