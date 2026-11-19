/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

#![allow(dead_code)] // TODO(rajneeshl): Remove this when the health checks are moved to the server.

use buck2_common::invocation_paths::InvocationPaths;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinHandle;

use crate::interface::HealthCheckContextEvent;
use crate::interface::HealthCheckEvent;
use crate::interface::HealthCheckService;
use crate::interface::HealthCheckSnapshotData;
use crate::report::DisplayReport;
use crate::report::Report;

/// An abstraction of health check clients. This allows us to test other parts of the code in isolation.
pub trait HealthCheckClient: Send + Sync {}

/// This client spawns a background task to read from events channel and forward to health check service.
pub struct StreamingHealthCheckClient {
    inner: JoinHandle<()>,
}

impl StreamingHealthCheckClient {
    pub fn new(
        tags_sender: Option<Sender<Vec<String>>>,
        display_reports_sender: Option<Sender<Vec<DisplayReport>>>,
        event_receiver: Receiver<HealthCheckEvent>,
        paths: Option<&InvocationPaths>,
    ) -> buck2_error::Result<Self> {
        let Some(path) = paths else {
            return Err(buck2_error::buck2_error!(
                buck2_error::ErrorTag::HealthCheck,
                "Error setting up health check state dir. Health checks will not be run."
            ));
        };
        let health_check_dir = path.health_check_state_dir();
        let inner = tokio::spawn(async move {
            let mut client =
                HealthCheckClientInner::new(tags_sender, display_reports_sender, health_check_dir);
            client.run_event_loop(event_receiver).await;
        });
        Ok(Self { inner })
    }
}

impl HealthCheckClient for StreamingHealthCheckClient {}

impl Drop for StreamingHealthCheckClient {
    fn drop(&mut self) {
        self.inner.abort();
    }
}

struct HealthCheckClientInner {
    health_check_service: Box<dyn HealthCheckService>,
    // Writer to send tags to be logged to scuba.
    tags_sender: Option<Sender<Vec<String>>>,
    // Writer to send health check reports to be displayed to the user.
    display_reports_sender: Option<Sender<Vec<DisplayReport>>>,
}

impl HealthCheckClientInner {
    fn new(
        tags_sender: Option<Sender<Vec<String>>>,
        display_reports_sender: Option<Sender<Vec<DisplayReport>>>,
        health_check_dir: AbsNormPathBuf,
    ) -> Self {
        let health_check_service = Self::create_service(health_check_dir);
        Self::new_with_service(tags_sender, display_reports_sender, health_check_service)
    }

    fn new_with_service(
        tags_sender: Option<Sender<Vec<String>>>,
        display_reports_sender: Option<Sender<Vec<DisplayReport>>>,
        health_check_service: Box<dyn HealthCheckService>,
    ) -> Self {
        Self {
            health_check_service,
            tags_sender,
            display_reports_sender,
        }
    }

    fn create_service(health_check_dir: AbsNormPathBuf) -> Box<dyn HealthCheckService> {
        #[cfg(fbcode_build)]
        {
            Box::new(
                crate::service::health_check_rpc_client::HealthCheckRpcClient::new(
                    health_check_dir,
                ),
            )
        }
        #[cfg(not(fbcode_build))]
        {
            // There is no easy binary distribution mechanism for OSS, hence default to in-process execution.
            Box::new(
                crate::service::health_check_in_process_service::HealthCheckInProcessService::new(
                    health_check_dir,
                ),
            )
        }
    }

    async fn run_event_loop(&mut self, mut event_receiver: Receiver<HealthCheckEvent>) {
        while let Some(event) = event_receiver.recv().await {
            match event {
                HealthCheckEvent::HealthCheckContextEvent(event) => {
                    self.update_context(event).await;
                }
                HealthCheckEvent::Snapshot(snapshot) => {
                    self.run_checks(snapshot).await;
                }
            }
        }
    }

    async fn update_context(&mut self, event: HealthCheckContextEvent) {
        if let Err(e) = self.health_check_service.update_context(event).await {
            tracing::debug!("Health check context update failed: {:#}", e);
        }
    }

    async fn run_checks(&mut self, snapshot: HealthCheckSnapshotData) {
        match self.health_check_service.run_checks(snapshot).await {
            Ok(reports) => {
                if let Err(e) = self.send_reports(reports) {
                    tracing::debug!("Health check reporting failed: {:#}", e);
                }
            }
            Err(e) => {
                tracing::debug!("Health check run failed: {:#}", e);
            }
        }
    }

    fn send_reports(&mut self, reports: Vec<Report>) -> buck2_error::Result<()> {
        let mut tags: Vec<String> = Vec::new();
        let mut display_reports: Vec<DisplayReport> = Vec::new();

        for report in reports {
            if let Some(tag) = report.tag {
                tags.push(tag);
            }

            if let Some(display_report) = report.display_report {
                display_reports.push(display_report);
            }
        }
        self.send_tags(tags)?;
        self.send_display_reports(display_reports)?;
        Ok(())
    }

    fn send_tags(&mut self, tags: Vec<String>) -> buck2_error::Result<()> {
        if tags.is_empty() {
            // Since tags are aggregated and written to logs only once, we don't need to publish empty lists.
            return Ok(());
        }
        if let Some(tags_sender) = &mut self.tags_sender {
            match tags_sender.try_send(tags) {
                Err(TrySendError::Closed(_)) => {
                    // If the receiver is dropped, drop the sender to stop sending next time.
                    self.tags_sender = None;
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::HealthCheck,
                        "Health check tags listener closed."
                    ));
                }
                Err(TrySendError::Full(_)) => {
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::HealthCheck,
                        "Health check tags channel full. Dropping tags."
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn send_display_reports(
        &mut self,
        display_reports: Vec<DisplayReport>,
    ) -> buck2_error::Result<()> {
        // Even if there are no reports, publish an empty list to signify that we have run the health checks.
        if let Some(display_reports_sender) = &mut self.display_reports_sender {
            match display_reports_sender.try_send(display_reports) {
                Err(TrySendError::Closed(_)) => {
                    // If the receiver is dropped, drop the sender to stop sending next time.
                    self.display_reports_sender = None;
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::HealthCheck,
                        "Health check display reports listener closed."
                    ));
                }
                Err(TrySendError::Full(_)) => {
                    // If the channel is full, skip sending these reports rather than OOMing due to huge buffers.
                    return Err(buck2_error::buck2_error!(
                        buck2_error::ErrorTag::HealthCheck,
                        "Health check display reports channel full. Dropping reports."
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::SystemTime;

    use async_trait::async_trait;
    use tokio::sync::mpsc;

    use super::*;
    use crate::interface::HealthCheckType;
    use crate::report::HealthIssue;
    use crate::report::Message;
    use crate::report::Severity;

    struct TestHealthCheckService {
        reports: Arc<Mutex<Vec<Report>>>,
    }

    #[async_trait]
    impl HealthCheckService for TestHealthCheckService {
        async fn update_context(
            &mut self,
            _event: HealthCheckContextEvent,
        ) -> buck2_error::Result<()> {
            Ok(())
        }

        async fn run_checks(
            &mut self,
            _snapshot: HealthCheckSnapshotData,
        ) -> buck2_error::Result<Vec<Report>> {
            let reports = self.reports.lock().unwrap().clone();
            Ok(reports)
        }
    }

    #[tokio::test]
    async fn test_health_check_client() {
        // Create test reports
        let test_reports = vec![
            Report {
                tag: Some("test_tag_1".to_owned()),
                display_report: Some(DisplayReport {
                    health_check_type: HealthCheckType::StableRevision,
                    health_issue: Some(HealthIssue {
                        severity: Severity::Warning,
                        message: Message::Simple("Test report 1".to_owned()),
                        remediation: None,
                    }),
                }),
            },
            Report {
                tag: Some("test_tag_2".to_owned()),
                display_report: Some(DisplayReport {
                    health_check_type: HealthCheckType::LowDiskSpace,
                    health_issue: Some(HealthIssue {
                        severity: Severity::Info,
                        message: Message::Simple("Test report 2".to_owned()),
                        remediation: None,
                    }),
                }),
            },
        ];

        // Create channels
        let (event_sender, event_receiver) = mpsc::channel(10);
        let (display_reports_sender, mut display_reports_receiver) = mpsc::channel(10);
        let (tags_sender, mut tags_receiver) = mpsc::channel(10);

        // Create test service
        let reports = Arc::new(Mutex::new(test_reports.clone()));
        let test_service = Box::new(TestHealthCheckService {
            reports: reports.clone(),
        });

        // Create client with test service
        let mut client = HealthCheckClientInner::new_with_service(
            Some(tags_sender),
            Some(display_reports_sender),
            test_service,
        );

        // Spawn client event loop
        let client_handle = tokio::spawn(async move {
            client.run_event_loop(event_receiver).await;
        });

        // Send snapshot event
        event_sender
            .send(HealthCheckEvent::Snapshot(HealthCheckSnapshotData {
                timestamp: SystemTime::now(),
            }))
            .await
            .unwrap();

        // Wait for reports to be processed
        let received_tags =
            tokio::time::timeout(std::time::Duration::from_millis(100), tags_receiver.recv())
                .await
                .expect("Timed out waiting for tags")
                .expect("No tags received");

        let received_reports = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            display_reports_receiver.recv(),
        )
        .await
        .expect("Timed out waiting for display reports")
        .expect("No display reports received");

        // Verify received data
        assert_eq!(received_tags.len(), 2);
        assert!(received_tags.contains(&"test_tag_1".to_owned()));
        assert!(received_tags.contains(&"test_tag_2".to_owned()));

        assert_eq!(received_reports.len(), 2);
        assert_eq!(
            received_reports[0].health_check_type,
            HealthCheckType::StableRevision
        );
        assert_eq!(
            received_reports[1].health_check_type,
            HealthCheckType::LowDiskSpace
        );
        assert!(received_reports[0].health_issue.is_some());
        assert!(received_reports[1].health_issue.is_some());

        // Clean up
        drop(event_sender);
        client_handle.await.unwrap();
    }
}
