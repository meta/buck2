/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(dead_code)]
// TODO(rajneeshl): Remove this when we start forwarding events to client
use std::sync::Arc;

use async_trait::async_trait;
use buck2_common::invocation_paths::InvocationPaths;
use buck2_core::soft_error_or_panic;
use buck2_data::buck_event::Data::*;
use buck2_error::BuckErrorOptionContext;
use buck2_events::BuckEvent;
use buck2_health_check::health_check_client::HealthCheckClient;
use buck2_health_check::health_check_client::StreamingHealthCheckClient;
use buck2_health_check::interface::HealthCheckContextEvent;
use buck2_health_check::interface::HealthCheckEvent;
use buck2_health_check::report::DisplayReport;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;

use crate::subscribers::subscriber::EventSubscriber;

const EVENT_CHANNEL_SIZE: usize = 100;
const MAXIMUM_HEALTH_CHECK_EVENT_COUNT: u32 = 3600; // 1 hour of build events at expected rate of ~1/s.

struct HealthCheckEventStats {
    total_event_count: u32,
    excess_cache_miss_reported: bool,
}

/// This subscriber is responsible for forwarding events to the health check client
pub struct HealthCheckSubscriber {
    health_check_client: Option<Box<dyn HealthCheckClient>>,
    event_sender: Option<Sender<HealthCheckEvent>>,
    event_stats: HealthCheckEventStats,
}

#[async_trait]
impl EventSubscriber for HealthCheckSubscriber {
    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        for ev in events {
            self.handle_event(ev).await?;
        }
        Ok(())
    }
}

impl HealthCheckSubscriber {
    pub fn new(
        tags_sender: Sender<Vec<String>>,
        display_reports_sender: Sender<Vec<DisplayReport>>,
        paths: Option<&InvocationPaths>,
    ) -> Box<Self> {
        let (tx, rx) = tokio::sync::mpsc::channel(EVENT_CHANNEL_SIZE);
        let client = StreamingHealthCheckClient::new(
            Some(tags_sender),
            Some(display_reports_sender),
            rx,
            paths,
        )
        .map(|c| Box::new(c) as Box<dyn HealthCheckClient>)
        .ok();
        Self::new_with_client(client, tx)
    }

    fn new_with_client(
        health_check_client: Option<Box<dyn HealthCheckClient>>,
        events_tx: Sender<HealthCheckEvent>,
    ) -> Box<Self> {
        Box::new(Self {
            health_check_client,
            event_sender: Some(events_tx),
            event_stats: HealthCheckEventStats {
                total_event_count: 0,
                excess_cache_miss_reported: false,
            },
        })
    }

    async fn handle_event(&mut self, event: &Arc<BuckEvent>) -> buck2_error::Result<()> {
        if self.event_sender.is_none() || self.health_check_client.is_none() {
            return Ok(());
        }
        let trace_id = event.trace_id();

        let health_check_event = match event.data() {
            SpanStart(start) => match &start.data {
                Some(buck2_data::span_start_event::Data::Command(command)) => {
                    Some(HealthCheckEvent::HealthCheckContextEvent(
                        HealthCheckContextEvent::CommandStart(
                            buck2_data::CommandStartWithTraceId {
                                trace_id: trace_id.map(|id| id.to_string()).unwrap_or_default(),
                                command_start: Some(command.clone()),
                                timestamp: Some(event.timestamp().into()),
                            },
                        ),
                    ))
                }
                _ => None,
            },
            SpanEnd(end) => {
                use buck2_data::span_end_event::Data::*;
                match end
                    .data
                    .as_ref()
                    .internal_error("Missing `data` in SpanEnd")?
                {
                    FileWatcher(file_watcher) => file_watcher
                        .stats
                        .as_ref()
                        .and_then(|stats| stats.branched_from_revision.as_ref())
                        .map(|merge_base| {
                            HealthCheckEvent::HealthCheckContextEvent(
                                HealthCheckContextEvent::BranchedFromRevision(merge_base.clone()),
                            )
                        }),
                    ActionExecution(action_execution_end) => {
                        if self.event_stats.excess_cache_miss_reported {
                            None
                        } else {
                            let has_excess_cache_miss = action_execution_end
                                .invalidation_info
                                .as_ref()
                                .is_some_and(|v| v.changed_file.is_none());

                            has_excess_cache_miss.then(|| {
                                self.event_stats.excess_cache_miss_reported = true;
                                HealthCheckEvent::HealthCheckContextEvent(
                                    HealthCheckContextEvent::HasExcessCacheMisses(),
                                )
                            })
                        }
                    }
                    _ => None,
                }
            }
            Instant(instant) => {
                use buck2_data::instant_event::Data::*;
                match instant
                    .data
                    .as_ref()
                    .internal_error("Missing `data` in `Instant`")?
                {
                    SystemInfo(system_info) => Some(HealthCheckEvent::HealthCheckContextEvent(
                        HealthCheckContextEvent::ExperimentConfigurations(system_info.clone()),
                    )),
                    TargetPatterns(target_patterns) => {
                        Some(HealthCheckEvent::HealthCheckContextEvent(
                            HealthCheckContextEvent::ParsedTargetPatterns(target_patterns.clone()),
                        ))
                    }
                    Snapshot(_snapshot) => {
                        // Create a new HealthCheckSnapshotData from the snapshot
                        let snapshot_data =
                            buck2_health_check::interface::HealthCheckSnapshotData {
                                timestamp: event.timestamp(),
                            };
                        Some(HealthCheckEvent::Snapshot(snapshot_data))
                    }
                    _ => None,
                }
            }
            _ => None,
        };

        if let (Some(health_check_event), Some(event_sender)) =
            (health_check_event, &mut self.event_sender)
        {
            // If this is a CommandStart and the test override env var is set,
            // also send the threshold so the health check CLI process can use it
            // (the CLI is a separate process that doesn't inherit client env vars).
            let send_test_threshold = matches!(
                &health_check_event,
                HealthCheckEvent::HealthCheckContextEvent(HealthCheckContextEvent::CommandStart(_))
            );
            match event_sender.try_send(health_check_event) {
                Ok(_) => {}
                Err(TrySendError::Full(_)) => {
                    self.close_client_connection_with_error_report(
                        "Health check event channel full. Disabling health checks.",
                    );
                }
                Err(TrySendError::Closed(_)) => {
                    self.close_client_connection_with_error_report(
                        "Health check event receiver closed. Disabling health checks.",
                    );
                }
            }
            if send_test_threshold {
                if let Ok(val) = std::env::var("BUCK2_TEST_SLOW_BUILD_CHECK") {
                    if let (Ok(secs), Some(sender)) = (val.parse::<u64>(), &mut self.event_sender) {
                        drop(sender.try_send(HealthCheckEvent::HealthCheckContextEvent(
                            HealthCheckContextEvent::TestSlowBuildThreshold(secs),
                        )));
                    }
                }
            }
            self.event_stats.total_event_count += 1;

            if self.event_stats.total_event_count > MAXIMUM_HEALTH_CHECK_EVENT_COUNT {
                self.close_client_connection_with_error_report(&format!(
                "Too many health check events sent. Disabling health checks. Events sent: {}. Maximum allowed: {}",
                self.event_stats.total_event_count,
                MAXIMUM_HEALTH_CHECK_EVENT_COUNT
            ));
            }
        }
        Ok(())
    }

    pub fn close_client_connection_with_error_report(&mut self, error: &str) {
        self.event_sender.take();
        self.health_check_client.take();
        soft_error_or_panic!(
            "health_check_subscriber_error",
            buck2_error::buck2_error!(buck2_error::ErrorTag::HealthCheck, "{}", error)
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use buck2_data::ActionExecutionEnd;
    use buck2_data::ActionKind;
    use buck2_health_check::interface::HealthCheckType;
    use buck2_health_check::report::DisplayReport;
    use buck2_health_check::report::HealthIssue;
    use buck2_health_check::report::Message;
    use buck2_health_check::report::Severity;
    use buck2_wrapper_common::invocation_id::TraceId;
    use tokio::sync::mpsc::Receiver;
    use tokio::sync::mpsc::{self};
    use tokio::task::JoinHandle;

    use super::*;

    struct TestHealthCheckClient {
        handle: JoinHandle<()>,
    }

    impl TestHealthCheckClient {
        fn new(
            tags_tx: Sender<Vec<String>>,
            display_reports_tx: Sender<Vec<DisplayReport>>,
            mut event_rx: Receiver<HealthCheckEvent>,
        ) -> Self {
            let handle = tokio::spawn(async move {
                while let Some(event) = event_rx.recv().await {
                    match event {
                        HealthCheckEvent::Snapshot(_) => {
                            // Send test tags
                            let _unused = tags_tx
                                .send(vec!["test_tag1".to_owned(), "test_tag2".to_owned()])
                                .await;

                            let _unused = display_reports_tx.send(test_reports()).await;
                        }
                        HealthCheckEvent::HealthCheckContextEvent(_) => {
                            // Process context events if needed for tests
                        }
                    }
                }
            });
            Self { handle }
        }
    }

    impl HealthCheckClient for TestHealthCheckClient {}
    struct NoOpHealthCheckClient {}
    impl HealthCheckClient for NoOpHealthCheckClient {}

    fn test_reports() -> Vec<DisplayReport> {
        vec![
            DisplayReport {
                health_check_type: HealthCheckType::StableRevision,
                health_issue: Some(HealthIssue {
                    severity: Severity::Warning,
                    message: Message::Simple("Test report 1".to_owned()),
                    remediation: None,
                }),
            },
            DisplayReport {
                health_check_type: HealthCheckType::LowDiskSpace,
                health_issue: Some(HealthIssue {
                    severity: Severity::Info,
                    message: Message::Simple("Test report 2".to_owned()),
                    remediation: None,
                }),
            },
        ]
    }

    fn test_event(data: buck2_data::buck_event::Data) -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            data,
        ))
    }

    fn event_for_excess_cache_miss() -> buck2_data::buck_event::Data {
        let action_end = ActionExecutionEnd {
            kind: ActionKind::Run as i32,
            invalidation_info: Some(buck2_data::CommandInvalidationInfo {
                changed_file: None,
                changed_any: None,
            }),
            ..Default::default()
        };
        buck2_data::buck_event::Data::SpanEnd(buck2_data::SpanEndEvent {
            data: Some(buck2_data::span_end_event::Data::ActionExecution(Box::new(
                action_end,
            ))),
            ..buck2_data::SpanEndEvent::default()
        })
    }

    #[tokio::test]
    async fn test_health_check_subscriber() -> buck2_error::Result<()> {
        let (tags_tx, mut tags_rx) = mpsc::channel::<Vec<String>>(10);
        let (reports_tx, mut reports_rx) = mpsc::channel::<Vec<DisplayReport>>(10);
        let (events_tx, events_rx) = mpsc::channel::<HealthCheckEvent>(EVENT_CHANNEL_SIZE);

        // Create client and subscriber
        let client = TestHealthCheckClient::new(tags_tx, reports_tx, events_rx);
        let mut subscriber =
            HealthCheckSubscriber::new_with_client(Some(Box::new(client)), events_tx);

        // Create a snapshot event
        let event = test_event(
            buck2_data::InstantEvent {
                data: Some(Box::new(buck2_data::Snapshot::default()).into()),
            }
            .into(),
        );

        // Send the event
        subscriber.handle_event(&event).await?;

        // Verify tags were received
        let tags =
            tokio::time::timeout(std::time::Duration::from_millis(100), tags_rx.recv()).await?;
        assert_eq!(tags.unwrap(), vec!["test_tag1", "test_tag2"]);

        // Verify reports were received
        let reports =
            tokio::time::timeout(std::time::Duration::from_millis(100), reports_rx.recv()).await?;
        assert!(reports.is_some());
        let reports = reports.unwrap();
        assert_eq!(reports.len(), 2);
        assert!(reports[0].health_issue.is_some());
        assert!(reports[1].health_issue.is_some());

        Ok(())
    }

    #[tokio::test]
    async fn test_excess_cache_miss_reporting() {
        let (events_tx, mut events_rx) = mpsc::channel::<HealthCheckEvent>(EVENT_CHANNEL_SIZE);

        let mut subscriber = HealthCheckSubscriber::new_with_client(
            Some(Box::new(NoOpHealthCheckClient {})),
            events_tx,
        );

        let event1 = test_event(event_for_excess_cache_miss());
        let event2 = test_event(event_for_excess_cache_miss());

        subscriber.handle_event(&event1).await.unwrap();
        subscriber.handle_event(&event2).await.unwrap();

        // Verify that only one HealthCheckContextEvent::HasExcessCacheMisses is sent
        let mut buffer: Vec<HealthCheckEvent> = Vec::with_capacity(2);
        assert_eq!(1, events_rx.recv_many(&mut buffer, 2).await);
    }

    #[tokio::test]
    async fn test_event_limit_exceeded() -> buck2_error::Result<()> {
        let (events_tx, mut events_rx) = mpsc::channel::<HealthCheckEvent>(EVENT_CHANNEL_SIZE);

        let mut subscriber = HealthCheckSubscriber::new_with_client(
            Some(Box::new(NoOpHealthCheckClient {})),
            events_tx,
        );

        let event = test_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(Box::new(buck2_data::Snapshot::default()).into()),
            },
        ));

        for _ in 0..MAXIMUM_HEALTH_CHECK_EVENT_COUNT {
            subscriber.handle_event(&event).await?;
            let _ignored = events_rx.recv().await; // Consume from channel.
        }

        // Verify that the client and channel are still active
        assert!(subscriber.event_sender.is_some());
        assert!(subscriber.health_check_client.is_some());

        // Send the 3601st event
        subscriber.handle_event(&event).await?;

        // The event is sent on the channel and then the sender/client are dropped.
        let _ignored = events_rx.recv().await;
        assert!(subscriber.event_sender.is_none());
        assert!(subscriber.health_check_client.is_none());

        // Verify that no more events are sent
        let mut buffer: Vec<HealthCheckEvent> = Vec::with_capacity(1);
        assert_eq!(0, events_rx.recv_many(&mut buffer, 1).await);
        Ok(())
    }

    #[tokio::test]
    async fn test_slow_client() -> buck2_error::Result<()> {
        let (events_tx, events_rx_guard) = mpsc::channel::<HealthCheckEvent>(EVENT_CHANNEL_SIZE);

        let mut subscriber = HealthCheckSubscriber::new_with_client(
            Some(Box::new(NoOpHealthCheckClient {})),
            events_tx,
        );

        let event = test_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(Box::new(buck2_data::Snapshot::default()).into()),
            },
        ));

        for _ in 0..EVENT_CHANNEL_SIZE {
            subscriber.handle_event(&event).await?;
        }

        // Verify that the client and channel are still active
        assert!(subscriber.event_sender.is_some());
        assert!(subscriber.health_check_client.is_some());

        subscriber.handle_event(&event).await?;

        // Verify that the client and channel sender are dropped since the buffer is full.
        assert!(subscriber.event_sender.is_none());
        assert!(subscriber.health_check_client.is_none());
        drop(events_rx_guard);
        Ok(())
    }

    #[tokio::test]
    async fn test_dropped_client() -> buck2_error::Result<()> {
        let (events_tx, events_rx_guard) = mpsc::channel::<HealthCheckEvent>(EVENT_CHANNEL_SIZE);

        let mut subscriber = HealthCheckSubscriber::new_with_client(
            Some(Box::new(NoOpHealthCheckClient {})),
            events_tx,
        );

        let event = test_event(buck2_data::buck_event::Data::Instant(
            buck2_data::InstantEvent {
                data: Some(Box::new(buck2_data::Snapshot::default()).into()),
            },
        ));

        subscriber.handle_event(&event).await?;
        // Verify that the client and channel are still active
        assert!(subscriber.event_sender.is_some());
        assert!(subscriber.health_check_client.is_some());

        // Close channel receiver.
        drop(events_rx_guard);

        subscriber.handle_event(&event).await?;
        // Verify that the client and channel sender are dropped since the receiver is closed.
        assert!(subscriber.event_sender.is_none());
        assert!(subscriber.health_check_client.is_none());
        Ok(())
    }
}
