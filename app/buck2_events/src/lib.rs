/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//!
//! Events and event streams for Buck2.
//!
//! The `Event` enum is the set of events that Buck2 can produce. Events can be produced both during the course of
//! a command or through background operation (such as DICE invalidations of changed files).
//!
//! There are three critical nouns in this data model:
//!  * An **event**, which is a structure representing a point-in-time occurrence, with some additional data.
//!  * A **trace**, which is a collection of events that are to be interpreted as semantically linked. Traces are
//!    globally identified by a trace ID, which is a v4 UUID. Traces are often associated with individual commands,
//!    although they do not have to.
//!  * A **span**, which is a pair of two events that represent a start and stop pair. A span covers a range of time
//!    points. All events are parented to a span that was currently active at the location the event was emitted.

pub mod daemon_id;
pub mod dispatch;
pub mod metadata;
pub mod schedule_type;
pub mod sink;
pub mod source;
pub mod span;

use std::num::NonZeroU64;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_cli_proto::CommandResult;
use buck2_cli_proto::PartialResult;
use buck2_wrapper_common::invocation_id::TraceId;
use derive_more::From;
use serde::Serialize;

use crate::sink::channel::ChannelEventSink;
use crate::source::ChannelEventSource;
use crate::span::SpanId;

/// An event that can be produced by Buck2. Events are points in time with additional metadata attached to them,
/// depending on the nature of the event.
///
/// Some events are special in that they represent points in time where an operation started or ended. These events
/// introduce new "spans". All events belong to a span except the first and last events of a trace. All spans except
/// the span created by the first and last events of the trace have a parent; as such, spans form a tree.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BuckEvent {
    /// Full event, the rest of the fields are caches.
    event: Box<buck2_data::BuckEvent>,

    /// A timestamp for when this event was emitted.
    timestamp: SystemTime,

    /// If this event starts a new span, the span ID assigned to this span, or None if this event is a leaf event
    /// that does not start a new span.
    pub span_id: Option<SpanId>,

    /// The ID of the span that contains this event. Will be non-None in all Events except the first and last events
    /// of a trace.
    pub parent_id: Option<SpanId>,
}

impl BuckEvent {
    pub fn new(
        timestamp: SystemTime,
        trace_id: TraceId,
        span_id: Option<SpanId>,
        parent_id: Option<SpanId>,
        data: buck2_data::buck_event::Data,
    ) -> BuckEvent {
        let event = buck2_data::BuckEvent {
            timestamp: Some(timestamp.into()),
            trace_id: trace_id.to_string(),
            span_id: span_id.map_or(0, |s| s.0.into()),
            parent_id: parent_id.map_or(0, |s| s.0.into()),
            data: Some(data),
        };
        BuckEvent {
            event: Box::new(event),
            timestamp,
            span_id,
            parent_id,
        }
    }

    pub fn timestamp(&self) -> SystemTime {
        self.timestamp
    }

    pub fn trace_id(&self) -> buck2_error::Result<TraceId> {
        Ok(TraceId::from_str(&self.event.trace_id)?)
    }

    pub fn span_id(&self) -> Option<SpanId> {
        self.span_id
    }

    pub fn parent_id(&self) -> Option<SpanId> {
        self.parent_id
    }

    pub fn event(&self) -> &buck2_data::BuckEvent {
        &self.event
    }

    pub fn data(&self) -> &buck2_data::buck_event::Data {
        self.event
            .data
            .as_ref()
            .expect("data is set, it is validated")
    }

    pub fn data_mut(&mut self) -> &mut buck2_data::buck_event::Data {
        self.event
            .data
            .as_mut()
            .expect("data is set, it is validated")
    }

    pub fn span_start_event(&self) -> Option<&buck2_data::SpanStartEvent> {
        match self.data() {
            buck2_data::buck_event::Data::SpanStart(start) => Some(start),
            _ => None,
        }
    }

    pub fn span_end_event(&self) -> Option<&buck2_data::SpanEndEvent> {
        match self.data() {
            buck2_data::buck_event::Data::SpanEnd(end) => Some(end),
            _ => None,
        }
    }

    pub fn command_start(&self) -> buck2_error::Result<Option<&buck2_data::CommandStart>> {
        match self.span_start_event() {
            None => Ok(None),
            Some(span_start_event) => {
                match span_start_event
                    .data
                    .as_ref()
                    .ok_or_else(|| BuckEventError::MissingField(self.clone()))?
                {
                    buck2_data::span_start_event::Data::Command(command_start) => {
                        Ok(Some(command_start))
                    }
                    _ => Ok(None),
                }
            }
        }
    }
}

impl From<BuckEvent> for Box<buck2_data::BuckEvent> {
    fn from(e: BuckEvent) -> Self {
        e.event
    }
}

impl TryFrom<Box<buck2_data::BuckEvent>> for BuckEvent {
    type Error = buck2_error::Error;

    fn try_from(event: Box<buck2_data::BuckEvent>) -> buck2_error::Result<BuckEvent> {
        event.data.as_ref().ok_or(BuckEventError::MissingData)?;
        fn new_span_id(num: u64) -> Option<SpanId> {
            NonZeroU64::new(num).map(SpanId)
        }
        Ok(Self {
            timestamp: SystemTime::try_from(
                event.timestamp.ok_or(BuckEventError::MissingTimestamp)?,
            )?,
            span_id: new_span_id(event.span_id),
            parent_id: new_span_id(event.parent_id),
            event,
        })
    }
}

/// The set of events that can flow out of an EventSource.
#[derive(Debug, Clone, From)]
#[allow(clippy::large_enum_variant)]
pub enum Event {
    /// A command result, produced upon completion of a command.
    CommandResult(Box<CommandResult>),
    /// A progress event from this command. Different commands have different types.
    PartialResult(PartialResult),
    /// A regular buck event. Is the only type to end up in the Event Log
    Buck(BuckEvent),
}

impl Event {
    pub fn unpack_buck(&self) -> Option<&BuckEvent> {
        match self {
            Self::Buck(event) => Some(event),
            Self::CommandResult(..) | Self::PartialResult(..) => None,
        }
    }
}

/// Statistics from this event sink on how messages were processed.
#[derive(Clone, Debug, Default)]
pub struct EventSinkStats {
    /// Count of number of successful messages (e.g. those that have been processed by their downstream destination).
    pub successes: u64,
    // Count of messages that failed to be submitted and will not be retried.
    pub failures_invalid_request: u64,
    pub failures_unauthorized: u64,
    pub failures_rate_limited: u64,
    pub failures_pushed_back: u64,
    pub failures_enqueue_failed: u64,
    pub failures_internal_error: u64,
    pub failures_timed_out: u64,
    pub failures_unknown: u64,
    /// How many messages are currently buffered by this sink.
    pub buffered: u64,
    /// How many messages were not even enqueued by this sink.
    pub dropped: u64,
    /// How many bytes were written into this sink.
    pub bytes_written: u64,
    /// Microseconds the sink's worker spent on queued messages, converting them, uploading
    /// their files and waiting for the transport to take them; and how often it opened a
    /// transport. Zero for sinks without a worker.
    pub worker_busy_us: u64,
    pub worker_convert_us: u64,
    pub worker_upload_us: u64,
    pub worker_send_wait_us: u64,
    pub worker_transport_opens: u64,
    pub worker_upload_connect_us: u64,
    pub worker_upload_slot_wait_us: u64,
}

impl EventSinkStats {
    pub fn failures(&self) -> u64 {
        let EventSinkStats {
            successes: _,
            failures_invalid_request,
            failures_unauthorized,
            failures_rate_limited,
            failures_pushed_back,
            failures_enqueue_failed,
            failures_internal_error,
            failures_timed_out,
            failures_unknown,
            buffered: _,
            dropped: _,
            bytes_written: _,
            worker_busy_us: _,
            worker_convert_us: _,
            worker_upload_us: _,
            worker_send_wait_us: _,
            worker_transport_opens: _,
            worker_upload_connect_us: _,
            worker_upload_slot_wait_us: _,
        } = self;
        *failures_invalid_request
            + *failures_unauthorized
            + *failures_rate_limited
            + *failures_pushed_back
            + *failures_enqueue_failed
            + *failures_internal_error
            + *failures_timed_out
            + *failures_unknown
    }
}

/// A sink for events, easily plumbable to the guts of systems that intend to produce events consumeable by
/// higher-level clients. Sending an event is synchronous.
#[async_trait]
pub trait EventSink: Send + Sync {
    /// Sends an event into this sink, to be consumed elsewhere. Explicitly does not return a Result type; if sending
    /// an event does fail, implementations will handle the failure by panicking or performing some other graceful
    /// recovery; callers of EventSink are not expected to handle failures.
    fn send(&self, event: Event);

    /// Like `send`, but bypasses any internal buffering and delivers the event
    /// as directly as possible. For the scribe sink this means calling thrift
    /// synchronously instead of going through the producer queue. This is
    /// useful for high-priority events that must be delivered even under memory
    /// pressure. The default implementation falls back to `send`.
    async fn send_now(&self, event: Event) {
        self.send(event);
    }
}

#[async_trait]
pub trait EventSinkWithStats: Send + Sync {
    fn to_event_sync(self: Arc<Self>) -> Arc<dyn EventSink>;

    /// Collects stats on this sink (e.g. messages accepted, rejected).
    fn stats(&self) -> EventSinkStats;

    /// Deliver what is queued and finish the remote streams. Called once, at daemon shutdown,
    /// so an interrupted build ends on the remote side instead of staying open forever.
    async fn shutdown(&self) -> buck2_error::Result<()> {
        Ok(())
    }

    /// Deliver what is queued and wait, for at most `timeout`, until the remote end has
    /// acknowledged it, closing the streams that `scope` names. `None` for a sink with no
    /// remote streams.
    async fn drain(
        &self,
        _scope: DrainScope,
        _timeout: Duration,
    ) -> buck2_error::Result<Option<DrainReport>> {
        Ok(None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainScope {
    /// The streams of commands that ended. A running command's stream is sent what it holds
    /// and stays open.
    FinishedCommands,
    /// Every stream, a running command's marked interrupted, and no stream opens afterwards:
    /// the daemon is shutting down.
    Shutdown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamDrainOutcome {
    Acked,
    /// The deadline passed with `events` events not yet acknowledged, which the sink still
    /// holds and goes on delivering.
    Pending {
        events: u64,
    },
    Failed {
        events: u64,
        error: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamDrain {
    pub invocation_id: String,
    pub outcome: StreamDrainOutcome,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub streams: Vec<StreamDrain>,
}

impl DrainReport {
    pub fn is_complete(&self) -> bool {
        self.streams
            .iter()
            .all(|stream| stream.outcome == StreamDrainOutcome::Acked)
    }

    pub fn unacked_events(&self) -> u64 {
        self.streams
            .iter()
            .map(|stream| match &stream.outcome {
                StreamDrainOutcome::Acked => 0,
                StreamDrainOutcome::Pending { events } => *events,
                StreamDrainOutcome::Failed { events, .. } => *events,
            })
            .sum()
    }

    pub fn unacked_invocations(&self) -> Vec<&str> {
        self.streams
            .iter()
            .filter(|stream| stream.outcome != StreamDrainOutcome::Acked)
            .map(|stream| stream.invocation_id.as_str())
            .collect()
    }
}

#[async_trait]
impl EventSink for Arc<dyn EventSink> {
    fn send(&self, event: Event) {
        EventSink::send(self.as_ref(), event);
    }

    async fn send_now(&self, event: Event) {
        EventSink::send_now(self.as_ref(), event).await;
    }
}

/// Creates a pair of an EventSource and an EventSink such that writes to the sink can be read by the event source.
pub fn create_source_sink_pair() -> (ChannelEventSource, impl EventSink) {
    let (send, recv) = crossbeam_channel::unbounded();
    let sink = ChannelEventSink::new(send);
    let source = ChannelEventSource::new(recv);
    (source, sink)
}

#[allow(clippy::large_enum_variant)]
#[derive(buck2_error::Error, Debug)]
#[buck2(tag = InvalidEvent)]
enum BuckEventError {
    #[error("The `buck2_data::BuckEvent` provided has no `Timestamp`")]
    MissingTimestamp,
    #[error("The `buck2_data::BuckEvent` provided has no `Data`")]
    MissingData,
    #[error("Sent an event missing one or more fields: `{0:?}`")]
    MissingField(BuckEvent),
}

pub fn init_late_bindings() {
    buck2_core::event::EVENT_DISPATCH.init(&dispatch::EventDispatcherLateBinding);
}

#[cfg(test)]
mod tests {
    use buck2_data::CommandStart;
    use buck2_data::SpanStartEvent;

    use super::*;

    #[test]
    fn round_trip_success() {
        let test = BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            Some(SpanId::next()),
            Some(SpanId::next()),
            SpanStartEvent {
                data: Some(
                    CommandStart {
                        ..Default::default()
                    }
                    .into(),
                ),
            }
            .into(),
        );
        assert_eq!(
            test,
            BuckEvent::try_from(Box::<buck2_data::BuckEvent>::from(test.clone())).unwrap()
        );
    }

    #[test]
    fn trace_id_hash_produces_a_reasonable_number() {
        let trace_id = TraceId::from_str("0436430c-2b02-624c-2032-570501212b57").unwrap();
        let hash = trace_id.hash();
        assert_eq!(3365465628718372403, hash);

        let other_trace_id = TraceId::from_str("586615bb-f57a-45a6-8804-3c6fcb0347de").unwrap();
        let hash = other_trace_id.hash();
        assert_eq!(-386302638890495926, hash);
    }
}
