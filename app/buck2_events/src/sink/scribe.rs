/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_core::buck2_env;
use buck2_data::ActionExecutionEnd;
use buck2_data::InstantEvent;
use buck2_data::Location;
use buck2_data::StructuredError;
use buck2_error::ErrorTag;
use buck2_error::conversion::from_any_with_tag;
use buck2_util::truncate::truncate;
use fbinit::FacebookInit;
use prost::Message;
pub use scribe_client::ScribeConfig;

use crate::BuckEvent;
use crate::Event;
use crate::EventSink;
use crate::EventSinkStats;
use crate::EventSinkWithStats;
use crate::TraceId;
use crate::daemon_id::get_daemon_id_for_panics;
use crate::metadata;
use crate::schedule_type::SandcastleScheduleType;
use crate::sink::smart_truncate_event::smart_truncate_event;

// 1 MiB limit
static SCRIBE_MESSAGE_SIZE_LIMIT: usize = 1024 * 1024;
// 50k characters
static TRUNCATED_SCRIBE_MESSAGE_SIZE: usize = 50000;

/// RemoteEventSink is a ScribeSink backed by the Thrift-based client in the `buck2_scribe_client` crate.
pub struct RemoteEventSink {
    category: String,
    client: scribe_client::ScribeClient,
    schedule_type: SandcastleScheduleType,
}

impl RemoteEventSink {
    /// Creates a new RemoteEventSink that forwards messages onto the Thrift-backed Scribe client.
    pub fn new(
        fb: FacebookInit,
        category: String,
        config: ScribeConfig,
    ) -> buck2_error::Result<RemoteEventSink> {
        let client = scribe_client::ScribeClient::new(fb, config)
            .map_err(|e| from_any_with_tag(e, ErrorTag::Tier0))?;

        let schedule_type = SandcastleScheduleType::new()?;
        Ok(RemoteEventSink {
            category,
            client,
            schedule_type,
        })
    }

    // Send this event now, bypassing internal message queue.
    pub async fn send_now(&self, event: BuckEvent) -> buck2_error::Result<()> {
        self.send_messages_now(vec![event]).await
    }

    // Send multiple events now, bypassing internal message queue.
    pub async fn send_messages_now(&self, events: Vec<BuckEvent>) -> buck2_error::Result<()> {
        let messages = events
            .into_iter()
            .map(|e| {
                let message_key = message_key(&e);
                scribe_client::Message {
                    category: self.category.clone(),
                    message: Self::encode_message(e),
                    message_key: Some(message_key),
                }
            })
            .collect();
        self.client
            .send_messages_now(messages)
            .await
            .map_err(|e| from_any_with_tag(e, ErrorTag::Tier0))
    }

    // Send this event by placing it on the internal message queue.
    pub fn offer(&self, event: BuckEvent) {
        let message_key = message_key(&event);
        self.client.offer(scribe_client::Message {
            category: self.category.clone(),
            message: Self::encode_message(event),
            message_key: Some(message_key),
        });
    }

    // Encodes message into something scribe understands.
    fn encode_message(mut event: BuckEvent) -> Vec<u8> {
        smart_truncate_event(event.data_mut());
        let mut proto: Box<buck2_data::BuckEvent> = event.into();

        Self::prepare_event(&mut proto);

        let buf = proto.encode_to_vec();
        if buf.len() > SCRIBE_MESSAGE_SIZE_LIMIT {
            let json = serde_json::to_string(&proto).unwrap();

            let proto: Box<buck2_data::BuckEvent> = BuckEvent::new(
                    SystemTime::now(),
                    TraceId::new(),
                    None,
                    None,
                    buck2_data::buck_event::Data::Instant(InstantEvent {
                        data: Some(
                            StructuredError {
                                location: Some(Location {
                                    file: file!().to_owned(),
                                    line: line!(),
                                    column: column!(),
                                }),
                                payload: format!("Soft Error: oversized_scribe: Message is oversized. Event data: {}. Original message size: {}", truncate(&json, TRUNCATED_SCRIBE_MESSAGE_SIZE),
                                buf.len()),
                                metadata: metadata::collect(&get_daemon_id_for_panics()),
                                backtrace: Vec::new(),
                                quiet: false,
                                task: Some(true),
                                soft_error_category: Some(buck2_data::SoftError {category: "oversized_scribe".to_owned(), is_quiet:false, is_deprecation: false}),
                                daemon_in_memory_state_is_corrupted: false,
                                daemon_materializer_state_is_corrupted: false,
                                action_cache_is_corrupted: false,
                                error: None,
                                daemon_restart_requested: false,
                            }
                            .into(),
                        ),
                    }),
                ).into();

            proto.encode_to_vec()
        } else {
            buf
        }
    }

    fn prepare_event(event: &mut buck2_data::BuckEvent) {
        use buck2_data::buck_event::Data;

        if let Some(Data::SpanEnd(s)) = &mut event.data
            && let Some(buck2_data::span_end_event::Data::ActionExecution(action)) = &mut s.data
        {
            let mut is_cache_hit = false;

            for command in action.commands.iter_mut() {
                let Some(details) = command.details.as_mut() else {
                    continue;
                };

                if get_is_cache_hit(details) {
                    is_cache_hit = true;
                    details.metadata = None;
                }
            }

            if is_cache_hit {
                action.dep_file_key = None;
                action.outputs.clear();
            }
        }
    }

    fn should_send_event(&self, data: &buck2_data::buck_event::Data) -> bool {
        use buck2_data::buck_event::Data;

        match data {
            Data::SpanStart(s) => {
                use buck2_data::span_start_event::Data;

                match &s.data {
                    Some(Data::Command(..)) => true,
                    None => false,
                    _ => false,
                }
            }
            Data::SpanEnd(s) => {
                use buck2_data::ActionExecutionKind;
                use buck2_data::span_end_event::Data;

                match &s.data {
                    Some(Data::Command(..)) => true,
                    Some(Data::ActionExecution(a)) => {
                        a.failed
                            || match ActionExecutionKind::try_from(a.execution_kind) {
                                // Those kinds are not used in downstreams
                                Ok(ActionExecutionKind::Simple) => false,
                                Ok(ActionExecutionKind::Deferred) => false,
                                Ok(ActionExecutionKind::NotSet) => false,
                                _ => !matches!(
                                    (action_has_cache_hit(a), self.schedule_type.is_diff()),
                                    (true, true)
                                ),
                            }
                    }
                    Some(Data::Analysis(..)) => !self.schedule_type.is_diff(),
                    Some(Data::Load(..)) => true,
                    Some(Data::CacheUpload(..)) => true,
                    Some(Data::DepFileUpload(..)) => true,
                    Some(Data::Materialization(..)) => true,
                    Some(Data::TestDiscovery(..)) => true,
                    Some(Data::TestRun(..)) => true,
                    None => false,
                    _ => false,
                }
            }
            Data::Instant(i) => {
                use buck2_data::instant_event::Data;

                match i.data {
                    Some(Data::BuildGraphInfo(..)) => true,
                    Some(Data::RageResult(..)) => true,
                    Some(Data::ReSession(..)) => true,
                    Some(Data::StructuredError(..)) => true,
                    Some(Data::PersistEventLogSubprocess(..)) => true,
                    Some(Data::CleanStaleResult(..)) => true,
                    Some(Data::PageOutSummary(..)) => true,
                    Some(Data::ConfigurationCreated(..)) => true,
                    Some(Data::DetailedAggregatedMetrics(..)) => true,
                    Some(Data::ResourceControlEvent(..)) => true,
                    Some(Data::ActionDigestTrace(..)) => true,
                    None => false,
                    _ => false,
                }
            }
            Data::Record(r) => {
                use buck2_data::record_event::Data;

                match r.data {
                    Some(Data::InvocationRecord(..)) => true,
                    Some(Data::BuildGraphStats(..)) => true,
                    None => false,
                }
            }
        }
    }
}

#[async_trait]
impl EventSink for RemoteEventSink {
    fn send(&self, event: Event) {
        match event {
            Event::Buck(event) => {
                if self.should_send_event(event.data()) {
                    self.offer(event);
                }
            }
            Event::CommandResult(..) => {}
            Event::PartialResult(..) => {}
        }
    }

    async fn send_now(&self, event: Event) {
        match event {
            Event::Buck(event) => {
                if self.should_send_event(event.data()) {
                    let _ignored = RemoteEventSink::send_now(self, event).await;
                }
            }
            Event::CommandResult(..) => {}
            Event::PartialResult(..) => {}
        }
    }
}

/// The message key determines which Scribe consumer shard receives the
/// message.
///
/// Events are keyed by trace id, so that one consumer shard sees an entire
/// invocation. `ConfigurationCreated` events are the exception: they are keyed
/// by the configuration's name. The same configurations are recreated by many
/// invocations, and routing each configuration to a single consumer shard
/// makes the consumer's in-memory deduplication of the resulting database
/// writes effective fleet-wide, instead of once per consumer shard. Nothing
/// downstream correlates these events with the rest of their invocation.
fn message_key(event: &BuckEvent) -> i64 {
    if let buck2_data::buck_event::Data::Instant(instant) = event.data()
        && let Some(buck2_data::instant_event::Data::ConfigurationCreated(created)) = &instant.data
        && let Some(cfg) = &created.cfg
    {
        let mut hasher = DefaultHasher::new();
        cfg.full_name.hash(&mut hasher);
        return hasher.finish() as i64;
    }
    event.trace_id().unwrap().hash()
}

fn action_has_cache_hit(action: &ActionExecutionEnd) -> bool {
    for details in action.commands.iter().filter_map(|e| e.details.as_ref()) {
        if get_is_cache_hit(details) {
            return true;
        }
    }
    false
}

fn get_is_cache_hit(details: &buck2_data::CommandExecutionDetails) -> bool {
    use buck2_data::command_execution_kind::Command::RemoteCommand;

    details
        .command_kind
        .as_ref()
        .and_then(|v| {
            v.command.as_ref().map(|v| match v {
                RemoteCommand(remote) => remote.cache_hit,
                _ => false,
            })
        })
        .unwrap_or(false)
}

impl EventSinkWithStats for RemoteEventSink {
    fn to_event_sync(self: Arc<Self>) -> Arc<dyn EventSink> {
        self as _
    }

    fn stats(&self) -> EventSinkStats {
        let counters = self.client.export_counters();
        EventSinkStats {
            successes: counters.successes,
            failures_invalid_request: counters.failures_invalid_request,
            failures_unauthorized: counters.failures_unauthorized,
            failures_rate_limited: counters.failures_rate_limited,
            failures_pushed_back: counters.failures_pushed_back,
            failures_enqueue_failed: counters.failures_enqueue_failed,
            failures_internal_error: counters.failures_internal_error,
            failures_timed_out: counters.failures_timed_out,
            failures_unknown: counters.failures_unknown,
            buffered: counters.queue_depth,
            dropped: counters.dropped,
            bytes_written: counters.bytes_written,
        }
    }
}

pub(crate) fn scribe_category() -> buck2_error::Result<String> {
    const DEFAULT_SCRIBE_CATEGORY: &str = "buck2_events";
    // Note that both daemon and client are emitting events, and that changing this variable has
    // no effect on the daemon until buckd is restarted but has effect on the client.
    Ok(
        buck2_env!("BUCK2_SCRIBE_CATEGORY", applicability = internal)?
            .unwrap_or(DEFAULT_SCRIBE_CATEGORY)
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use buck2_data::CommandExecutionDetails;
    use buck2_data::CommandExecutionKind;
    use buck2_data::RemoteCommand;
    use buck2_data::command_execution_kind::Command;

    use super::*;

    #[test]
    fn test_encode_large_message() {
        let large_string = "x".repeat(2 * 1024 * 1024); // 2MB string, exceeding SCRIBE_MESSAGE_SIZE_LIMIT
        let event = BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            buck2_data::buck_event::Data::Instant(InstantEvent {
                data: Some(buck2_data::instant_event::Data::StructuredError(
                    buck2_data::StructuredError {
                        payload: large_string,
                        ..Default::default()
                    },
                )),
            }),
        );

        let res = RemoteEventSink::encode_message(event);
        let size_approx = res.len() * 8;
        assert!(size_approx > TRUNCATED_SCRIBE_MESSAGE_SIZE);
        assert!(size_approx < SCRIBE_MESSAGE_SIZE_LIMIT);
    }

    #[test]
    fn test_message_key() {
        let configuration_created = |name: &str| {
            BuckEvent::new(
                SystemTime::now(),
                TraceId::new(),
                None,
                None,
                buck2_data::buck_event::Data::Instant(InstantEvent {
                    data: Some(buck2_data::instant_event::Data::ConfigurationCreated(
                        buck2_data::ConfigurationCreated {
                            cfg: Some(buck2_data::ConfigurationWithConstraints {
                                full_name: name.to_owned(),
                                constraint: Vec::new(),
                            }),
                        },
                    )),
                }),
            )
        };

        // Same configuration from different invocations shards identically;
        // different configurations shard independently.
        let a = configuration_created("cfg:linux-x86_64#abc123");
        let b = configuration_created("cfg:linux-x86_64#abc123");
        let c = configuration_created("cfg:linux-x86_64#def456");
        assert_eq!(message_key(&a), message_key(&b));
        assert_ne!(message_key(&a), message_key(&c));

        // Everything else is keyed by its trace id.
        let trace = TraceId::new();
        let trace_key = trace.hash();
        let other = BuckEvent::new(
            SystemTime::now(),
            trace,
            None,
            None,
            buck2_data::buck_event::Data::Instant(InstantEvent {
                data: Some(buck2_data::instant_event::Data::StructuredError(
                    StructuredError::default(),
                )),
            }),
        );
        assert_eq!(message_key(&other), trace_key);
    }

    #[test]
    fn test_get_is_cache_hit() {
        // Case 1: Remote command with cache hit
        let details_cache_hit = CommandExecutionDetails {
            command_kind: Some(CommandExecutionKind {
                command: Some(Command::RemoteCommand(RemoteCommand {
                    cache_hit: true,
                    ..Default::default()
                })),
            }),
            ..Default::default()
        };
        assert!(get_is_cache_hit(&details_cache_hit));

        // Case 2: Remote command without cache hit
        let details_no_cache_hit = CommandExecutionDetails {
            command_kind: Some(CommandExecutionKind {
                command: Some(Command::RemoteCommand(RemoteCommand {
                    cache_hit: false,
                    ..Default::default()
                })),
            }),
            ..Default::default()
        };
        assert!(!get_is_cache_hit(&details_no_cache_hit));
    }
}
