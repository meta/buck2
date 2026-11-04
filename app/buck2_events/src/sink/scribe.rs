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
#[cfg(fbcode_build)]
use buck2_error::ErrorTag;
#[cfg(fbcode_build)]
use buck2_error::conversion::from_any_with_tag;
use buck2_util::truncate::truncate;
use fbinit::FacebookInit;
use prost::Message;
#[cfg(fbcode_build)]
pub use scribe_client::ScribeConfig as RemoteEventConfig;

use crate::BuckEvent;
use crate::Event;
use crate::EventSink;
use crate::EventSinkStats;
use crate::EventSinkWithStats;
use crate::TraceId;
use crate::daemon_id::get_daemon_id_for_panics;
use crate::metadata;
use crate::schedule_type::SandcastleScheduleType;
#[cfg(not(fbcode_build))]
pub use crate::sink::bes_client::BesConfig as RemoteEventConfig;
#[cfg(not(fbcode_build))]
pub use crate::sink::bes_client::BesEventFormat;
#[cfg(not(fbcode_build))]
use crate::sink::bes_client::PrioritySend;
use crate::sink::smart_truncate_event::smart_truncate_event;
#[cfg(not(fbcode_build))]
use crate::sink::smart_truncate_event::smart_truncate_event_preserving_logs;

/// How long a command waits for the sink's worker to send its CommandStart. The simple console
/// prints "Waiting on buck2 daemon" once 7 s pass with nothing to show
/// (buck2_client_ctx/src/subscribers/simpleconsole.rs:59), and a stalled sink must not read as a
/// stalled daemon. A healthy send is a TCP and TLS connection and one write, a few round trips.
#[cfg(not(fbcode_build))]
const COMMAND_START_SEND_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

// 1 MiB limit
static SCRIBE_MESSAGE_SIZE_LIMIT: usize = 1024 * 1024;
// 50k characters
static TRUNCATED_SCRIBE_MESSAGE_SIZE: usize = 50000;

/// RemoteEventSink forwards events to a remote backend.
pub struct RemoteEventSink {
    category: String,
    #[cfg(fbcode_build)]
    client: scribe_client::ScribeClient,
    #[cfg(not(fbcode_build))]
    client: crate::sink::bes_client::BesClient,
    #[cfg(not(fbcode_build))]
    upload_successful_action_events: bool,
    #[cfg(not(fbcode_build))]
    preserve_bazel_logs: bool,
    schedule_type: SandcastleScheduleType,
}

impl RemoteEventSink {
    /// Creates a new RemoteEventSink that forwards messages onto a remote backend.
    pub fn new(
        fb: FacebookInit,
        category: String,
        config: RemoteEventConfig,
    ) -> buck2_error::Result<RemoteEventSink> {
        #[cfg(fbcode_build)]
        let client = scribe_client::ScribeClient::new(fb, config)
            .map_err(|e| from_any_with_tag(e, ErrorTag::Tier0))?;
        #[cfg(not(fbcode_build))]
        let upload_successful_action_events = config.upload_successful_action_events;
        #[cfg(not(fbcode_build))]
        let preserve_bazel_logs = config.event_format == BesEventFormat::Bazel;
        #[cfg(not(fbcode_build))]
        let client = crate::sink::bes_client::BesClient::new(fb, config)?;

        let schedule_type = SandcastleScheduleType::new()?;
        Ok(RemoteEventSink {
            category,
            client,
            #[cfg(not(fbcode_build))]
            upload_successful_action_events,
            #[cfg(not(fbcode_build))]
            preserve_bazel_logs,
            schedule_type,
        })
    }

    // Send this event now, bypassing internal message queue.
    pub async fn send_now(&self, event: BuckEvent) -> buck2_error::Result<()> {
        self.send_messages_now(vec![event]).await
    }

    // Send multiple events now, bypassing internal message queue.
    pub async fn send_messages_now(&self, events: Vec<BuckEvent>) -> buck2_error::Result<()> {
        #[cfg(fbcode_build)]
        {
            let messages = events
                .into_iter()
                .map(|e| {
                    let message_key = message_key(&e);
                    scribe_client::Message {
                        category: self.category.clone(),
                        message: Self::encode_message(e, false),
                        message_key: Some(message_key),
                    }
                })
                .collect();
            self.client
                .send_messages_now(messages)
                .await
                .map_err(|e| from_any_with_tag(e, ErrorTag::Tier0))
        }
        #[cfg(not(fbcode_build))]
        {
            let messages = events
                .into_iter()
                .map(|e| {
                    let message_key = message_key(&e);
                    crate::sink::bes_client::Message {
                        category: self.category.clone(),
                        message: Self::encode_message(e, self.preserve_bazel_logs),
                        message_key: Some(message_key),
                    }
                })
                .collect();
            self.client.send_messages_now(messages).await
        }
    }

    #[cfg(not(fbcode_build))]
    fn send_before_client_output(&self, event: &BuckEvent) -> PrioritySend {
        let message_key = match event.trace_id() {
            Ok(trace_id) => trace_id.hash(),
            Err(e) => return PrioritySend::Done(Err(e)),
        };
        let message = crate::sink::bes_client::Message {
            category: self.category.clone(),
            message: Self::encode_message(event.clone(), self.preserve_bazel_logs),
            message_key: Some(message_key),
        };
        self.client
            .send_messages_within(vec![message], COMMAND_START_SEND_DEADLINE)
    }

    #[cfg(not(fbcode_build))]
    fn should_send_before_client_output(&self, event: &BuckEvent) -> bool {
        self.preserve_bazel_logs && is_command_start_event(event)
    }

    // Send this event by placing it on the internal message queue.
    pub fn offer(&self, event: BuckEvent) {
        let message_key = message_key(&event);
        #[cfg(fbcode_build)]
        self.client.offer(scribe_client::Message {
            category: self.category.clone(),
            message: Self::encode_message(event, false),
            message_key: Some(message_key),
        });
        #[cfg(not(fbcode_build))]
        self.client.offer(crate::sink::bes_client::Message {
            category: self.category.clone(),
            message: Self::encode_message(event, self.preserve_bazel_logs),
            message_key: Some(message_key),
        });
    }

    // Encodes message for transport.
    fn encode_message(mut event: BuckEvent, preserve_logs: bool) -> Vec<u8> {
        if preserve_logs {
            #[cfg(not(fbcode_build))]
            smart_truncate_event_preserving_logs(event.data_mut());
            #[cfg(fbcode_build)]
            smart_truncate_event(event.data_mut());
        } else {
            smart_truncate_event(event.data_mut());
        }
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

        #[cfg(not(fbcode_build))]
        {
            if let Some(Data::Instant(i)) = &mut event.data
                && let Some(buck2_data::instant_event::Data::BuckconfigInputValues(values)) =
                    &mut i.data
            {
                redact_sensitive_buckconfig_values(values);
            }
        }

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
        #[cfg(not(fbcode_build))]
        let upload_successful_action_events = self.upload_successful_action_events;
        #[cfg(fbcode_build)]
        let upload_successful_action_events = true;
        #[cfg(not(fbcode_build))]
        let preserve_bazel_logs = self.preserve_bazel_logs;
        #[cfg(fbcode_build)]
        let preserve_bazel_logs = false;

        should_send_event_data(
            data,
            &self.schedule_type,
            upload_successful_action_events,
            preserve_bazel_logs,
        )
    }
}

fn should_send_event_data(
    data: &buck2_data::buck_event::Data,
    schedule_type: &SandcastleScheduleType,
    upload_successful_action_events: bool,
    preserve_bazel_logs: bool,
) -> bool {
    use buck2_data::buck_event::Data;

    match data {
        Data::SpanStart(s) => {
            use buck2_data::span_start_event::Data;

            match &s.data {
                Some(Data::Command(..)) => true,
                // Buck2->BuildBuddy OSS integration needs this to capture
                // metadata sourced from buckconfigs.
                #[cfg(not(fbcode_build))]
                Some(Data::CommandCritical(..)) => true,
                // Buck2->BuildBuddy target mapping uses analysis start to
                // synthesize target configured events.
                #[cfg(not(fbcode_build))]
                Some(Data::Analysis(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(
                    Data::ActionExecution(..)
                    | Data::AnalysisStage(..)
                    | Data::ExecutorStage(..)
                    | Data::Load(..),
                ) if preserve_bazel_logs => true,
                None => false,
                _ => false,
            }
        }
        Data::SpanEnd(s) => {
            use buck2_data::span_end_event::Data;

            match &s.data {
                Some(Data::Command(..)) => true,
                Some(Data::ActionExecution(a)) => {
                    if preserve_bazel_logs {
                        return true;
                    }
                    should_send_action_execution(a, schedule_type, upload_successful_action_events)
                }
                Some(Data::Analysis(..)) => preserve_bazel_logs || !schedule_type.is_diff(),
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
                // Required by BuildBuddy Buck2 mapping for target/test
                // details and invocation metadata.
                #[cfg(not(fbcode_build))]
                Some(Data::TestDiscovery(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::TestResult(..)) => true,
                // The Bazel converter holds a test run's TestResult and TestSummary until the
                // run's per-test results arrive, and sends whatever is held at this one.
                #[cfg(not(fbcode_build))]
                Some(Data::EndOfTestResults(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::TargetPatterns(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::TargetCfg(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::CommandOptions(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::VersionControlRevision(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::BuckconfigInputValues(..)) => true,
                // Required for BuildBuddy build log rendering.
                #[cfg(not(fbcode_build))]
                Some(Data::ConsoleMessage(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::ConsoleWarning(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::StreamingOutput(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::ActionError(..)) => true,
                #[cfg(not(fbcode_build))]
                Some(Data::DiceStateSnapshot(..)) if preserve_bazel_logs => true,
                Some(Data::RageResult(..)) => true,
                Some(Data::ReSession(..)) => true,
                Some(Data::StructuredError(..)) => true,
                Some(Data::PersistEventLogSubprocess(..)) => true,
                Some(Data::CleanStaleResult(..)) => true,
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

fn should_send_action_execution(
    action: &ActionExecutionEnd,
    schedule_type: &SandcastleScheduleType,
    upload_successful_action_events: bool,
) -> bool {
    use buck2_data::ActionExecutionKind;

    if action.failed {
        return true;
    }

    if !upload_successful_action_events {
        return false;
    }

    match ActionExecutionKind::try_from(action.execution_kind) {
        // Those kinds are not used in downstreams.
        Ok(ActionExecutionKind::Simple) => false,
        Ok(ActionExecutionKind::Deferred) => false,
        Ok(ActionExecutionKind::NotSet) => false,
        _ => !matches!(
            (action_has_cache_hit(action), schedule_type.is_diff()),
            (true, true)
        ),
    }
}

#[async_trait]
impl EventSink for RemoteEventSink {
    fn send(&self, event: Event) {
        match event {
            Event::Buck(event) => {
                if self.should_send_event(event.data()) {
                    #[cfg(not(fbcode_build))]
                    if self.should_send_before_client_output(&event) {
                        // BuildBuddy creates the invocation row from the BEP
                        // Started/OptionsParsed batch generated from
                        // CommandStart. Send that batch before the local client
                        // sees CommandStart and prints the BuildBuddy URL. This
                        // waits for local send, not for a BuildBuddy ACK.
                        match self.send_before_client_output(&event) {
                            PrioritySend::Done(Ok(())) => {}
                            PrioritySend::Done(Err(_)) => self.offer(event),
                            PrioritySend::StillQueued => tracing::warn!(
                                "BES sink: CommandStart not sent within {:?}; the command goes on, and the event is sent ahead of the command's later events once the sink's worker is free",
                                COMMAND_START_SEND_DEADLINE
                            ),
                        }
                        return;
                    }
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

#[cfg(not(fbcode_build))]
fn is_command_start_event(event: &BuckEvent) -> bool {
    matches!(
        event.data(),
        buck2_data::buck_event::Data::SpanStart(span)
            if matches!(
                span.data.as_ref(),
                Some(buck2_data::span_start_event::Data::Command(_))
            )
    )
}

#[cfg(not(fbcode_build))]
fn redact_sensitive_buckconfig_values(values: &mut buck2_data::BuckconfigInputValues) {
    use buck2_data::buckconfig_component::Data;

    for component in &mut values.components {
        let Some(data) = component.data.as_mut() else {
            continue;
        };
        let config_values = match data {
            Data::ConfigValue(value) => std::slice::from_mut(value),
            Data::ConfigFile(config_file) => match config_file.data.as_mut() {
                Some(buck2_data::config_file::Data::GlobalExternalConfig(config)) => {
                    config.values.as_mut_slice()
                }
                _ => continue,
            },
            Data::GlobalExternalConfigFile(config) => config.values.as_mut_slice(),
        };

        for value in config_values {
            if is_sensitive_buckconfig_value(value) {
                value.value = "<redacted>".to_owned();
            }
        }
    }
}

#[cfg(not(fbcode_build))]
fn is_sensitive_buckconfig_value(value: &buck2_data::ConfigValue) -> bool {
    let section = value.section.to_ascii_lowercase();
    let key = value.key.to_ascii_lowercase();
    section.contains("credential")
        || section.contains("secret")
        || section.contains("token")
        || key.contains("credential")
        || key.contains("password")
        || key.contains("secret")
        || key.contains("token")
        || key.contains("api_key")
        || key.contains("api-key")
        || key == "header"
        || key == "http_headers"
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

#[async_trait]
impl EventSinkWithStats for RemoteEventSink {
    fn to_event_sync(self: Arc<Self>) -> Arc<dyn EventSink> {
        self as _
    }

    async fn shutdown(&self) -> buck2_error::Result<()> {
        #[cfg(not(fbcode_build))]
        {
            self.client.close_all_streams().await
        }
        #[cfg(fbcode_build)]
        {
            Ok(())
        }
    }

    async fn drain(
        &self,
        scope: crate::DrainScope,
        timeout: std::time::Duration,
    ) -> buck2_error::Result<Option<crate::DrainReport>> {
        #[cfg(not(fbcode_build))]
        {
            self.client.drain(scope, timeout).await.map(Some)
        }
        #[cfg(fbcode_build)]
        {
            let _unused = (scope, timeout);
            Ok(None)
        }
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
            worker_busy_us: counters.worker_busy_us,
            worker_convert_us: counters.worker_convert_us,
            worker_upload_us: counters.worker_upload_us,
            worker_send_wait_us: counters.worker_send_wait_us,
            worker_transport_opens: counters.worker_transport_opens,
            worker_upload_connect_us: counters.worker_upload_connect_us,
            worker_upload_slot_wait_us: counters.worker_upload_slot_wait_us,
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
    use buck2_data::ActionExecutionEnd;
    use buck2_data::ActionExecutionKind;
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

        let res = RemoteEventSink::encode_message(event, false);
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

    #[cfg(not(fbcode_build))]
    #[test]
    fn command_start_events_are_sent_before_client_output() {
        let command_start = BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                data: Some(buck2_data::CommandStart::default().into()),
            }),
        );
        assert!(is_command_start_event(&command_start));

        let console_message = BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                data: Some(
                    buck2_data::ConsoleMessage {
                        message: "hello".to_owned(),
                    }
                    .into(),
                ),
            }),
        );
        assert!(!is_command_start_event(&console_message));
    }

    #[cfg(not(fbcode_build))]
    #[test]
    fn command_start_does_not_wait_for_a_sink_held_by_a_silent_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for connection in listener.incoming().flatten() {
                held.push(connection);
            }
        });
        let sink = Arc::new(
            RemoteEventSink::new(
                // SAFETY: outside fbcode the token stands for nothing and nothing reads it.
                unsafe { fbinit::assume_init() },
                "test".to_owned(),
                RemoteEventConfig {
                    bes_backend: Some(format!("grpcs://{address}")),
                    event_format: BesEventFormat::Bazel,
                    // Longer than the test: the TLS handshake holds the sink's worker throughout.
                    grpc_timeout: std::time::Duration::from_secs(60),
                    ..RemoteEventConfig::default()
                },
            )
            .expect("sink"),
        );
        let command_start = || {
            Event::Buck(BuckEvent::new(
                SystemTime::now(),
                TraceId::new(),
                None,
                None,
                buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
                    data: Some(buck2_data::CommandStart::default().into()),
                }),
            ))
        };

        let earlier = command_start();
        let earlier_sink = sink.clone();
        std::thread::spawn(move || earlier_sink.send(earlier));
        std::thread::sleep(std::time::Duration::from_millis(100));

        let (sent_tx, sent_rx) = std::sync::mpsc::channel();
        let later = command_start();
        let later_sink = sink.clone();
        std::thread::spawn(move || {
            later_sink.send(later);
            let _ignored = sent_tx.send(());
        });
        assert!(
            sent_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok(),
            "a command's start waited more than 5 s on a BES worker held by a server that never answers"
        );
    }

    #[test]
    fn upload_successful_action_events_keeps_failures() {
        let schedule_type = SandcastleScheduleType::testing_empty();
        let failed_remote = ActionExecutionEnd {
            failed: true,
            execution_kind: ActionExecutionKind::Remote as i32,
            ..Default::default()
        };

        assert!(should_send_action_execution(
            &failed_remote,
            &schedule_type,
            false
        ));
    }

    #[test]
    fn upload_successful_action_events_filters_successes() {
        let schedule_type = SandcastleScheduleType::testing_empty();
        let successful_remote = ActionExecutionEnd {
            failed: false,
            execution_kind: ActionExecutionKind::Remote as i32,
            ..Default::default()
        };

        assert!(should_send_action_execution(
            &successful_remote,
            &schedule_type,
            true
        ));
        assert!(!should_send_action_execution(
            &successful_remote,
            &schedule_type,
            false
        ));
    }

    #[cfg(not(fbcode_build))]
    #[test]
    fn bazel_progress_keeps_successful_action_events_for_converter() {
        let schedule_type = SandcastleScheduleType::testing_empty();
        let data = buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                ActionExecutionEnd {
                    failed: false,
                    execution_kind: ActionExecutionKind::Remote as i32,
                    ..Default::default()
                },
            ))),
            ..Default::default()
        });

        assert!(should_send_event_data(&data, &schedule_type, false, true));
        assert!(!should_send_event_data(&data, &schedule_type, false, false));
    }

    #[cfg(not(fbcode_build))]
    #[test]
    fn bazel_progress_keeps_progress_source_events() {
        let schedule_type = SandcastleScheduleType::testing_empty();
        let action_start = buck2_data::buck_event::Data::SpanStart(buck2_data::SpanStartEvent {
            data: Some(buck2_data::span_start_event::Data::ActionExecution(
                buck2_data::ActionExecutionStart::default(),
            )),
        });
        let dice_snapshot = buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
            data: Some(buck2_data::instant_event::Data::DiceStateSnapshot(
                buck2_data::DiceStateSnapshot::default(),
            )),
        });

        assert!(should_send_event_data(
            &action_start,
            &schedule_type,
            false,
            true
        ));
        assert!(should_send_event_data(
            &dice_snapshot,
            &schedule_type,
            false,
            true
        ));
        assert!(!should_send_event_data(
            &action_start,
            &schedule_type,
            false,
            false
        ));
        assert!(!should_send_event_data(
            &dice_snapshot,
            &schedule_type,
            false,
            false
        ));
    }
}
