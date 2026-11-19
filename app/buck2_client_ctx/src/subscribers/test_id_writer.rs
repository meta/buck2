/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::sync::Arc;

use async_trait::async_trait;
use buck2_data::error::ErrorTag;
use buck2_error::BuckErrorContext;
use buck2_error::buck2_error;
use buck2_event_observer::unpack_event::UnpackedBuckEvent;
use buck2_event_observer::unpack_event::unpack_event;
use buck2_events::BuckEvent;
use buck2_fs::async_fs_util;
use buck2_fs::error::IoResultExt;
use buck2_fs::paths::abs_path::AbsPathBuf;

use crate::subscribers::subscriber::EventSubscriber;

pub struct TestIdWriter {
    path: AbsPathBuf,
    // EventSubscriber::handle_events takes &mut self, so exclusive access is guaranteed
    written: bool,
}

impl TestIdWriter {
    pub fn new(path: AbsPathBuf) -> Self {
        Self {
            path,
            written: false,
        }
    }
}

#[async_trait]
impl EventSubscriber for TestIdWriter {
    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        if self.written {
            return Ok(());
        }

        for event in events {
            if let Ok(UnpackedBuckEvent::Instant(
                _,
                _,
                buck2_data::instant_event::Data::TestDiscovery(discovery),
            )) = unpack_event(event)
            {
                if let Some(buck2_data::test_discovery::Data::Session(session)) = &discovery.data {
                    let test_id: &str = session
                        .test_session_id
                        .as_deref()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| {
                            buck2_error!(
                                ErrorTag::InvalidEvent,
                                "Test session did not include a test_session_id"
                            )
                        })?;
                    async_fs_util::write(&self.path, test_id)
                        .await
                        .categorize_input()
                        .buck_error_context("Error writing test ID")?;
                    self.written = true;
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::SystemTime;

    use buck2_events::BuckEvent;
    use buck2_fs::paths::abs_path::AbsPathBuf;
    use buck2_wrapper_common::invocation_id::TraceId;

    use super::*;

    fn test_discovery_session_event(info: &str, test_session_id: Option<&str>) -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                data: Some(
                    buck2_data::TestDiscovery {
                        data: Some(buck2_data::test_discovery::Data::Session(
                            buck2_data::TestSessionInfo {
                                info: info.to_owned(),
                                test_session_id: test_session_id.map(|s| s.to_owned()),
                            },
                        )),
                    }
                    .into(),
                ),
            }),
        ))
    }

    fn non_matching_event() -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
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
        ))
    }

    fn test_discovery_no_data_event() -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                data: Some(buck2_data::TestDiscovery { data: None }.into()),
            }),
        ))
    }

    fn test_discovery_suite_event() -> Arc<BuckEvent> {
        Arc::new(BuckEvent::new(
            SystemTime::now(),
            TraceId::new(),
            None,
            None,
            buck2_data::buck_event::Data::Instant(buck2_data::InstantEvent {
                data: Some(
                    buck2_data::TestDiscovery {
                        data: Some(buck2_data::test_discovery::Data::Tests(
                            buck2_data::TestSuite {
                                test_names: vec!["test_foo".to_owned()],
                                ..Default::default()
                            },
                        )),
                    }
                    .into(),
                ),
            }),
        ))
    }

    #[tokio::test]
    async fn test_writes_session_id() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        let event = test_discovery_session_event(
            "https://www.internalfb.com/intern/testinfra/testrun/10414574293803519",
            Some("10414574293803519"),
        );
        writer.handle_events(&[event]).await?;

        let content = tokio::fs::read_to_string(path.as_path()).await?;
        assert_eq!(content, "10414574293803519");
        Ok(())
    }

    #[tokio::test]
    async fn test_writes_only_once() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        let event1 = test_discovery_session_event("", Some("first_id"));
        let event2 = test_discovery_session_event("", Some("second_id"));
        writer.handle_events(&[event1]).await?;
        writer.handle_events(&[event2]).await?;

        let content = tokio::fs::read_to_string(path.as_path()).await?;
        assert_eq!(content, "first_id");
        Ok(())
    }

    #[tokio::test]
    async fn test_ignores_non_matching_events() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        let event = non_matching_event();
        writer.handle_events(&[event]).await?;

        assert!(!path.as_path().exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_finds_session_in_mixed_batch() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        writer
            .handle_events(&[
                non_matching_event(),
                test_discovery_no_data_event(),
                test_discovery_suite_event(),
                test_discovery_session_event("", Some("found_it")),
            ])
            .await?;

        let content = tokio::fs::read_to_string(path.as_path()).await?;
        assert_eq!(content, "found_it");
        Ok(())
    }

    #[tokio::test]
    async fn test_writes_first_session_in_batch() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        writer
            .handle_events(&[
                test_discovery_session_event("", Some("first")),
                test_discovery_session_event("", Some("second")),
            ])
            .await?;

        let content = tokio::fs::read_to_string(path.as_path()).await?;
        assert_eq!(content, "first");
        Ok(())
    }

    #[tokio::test]
    async fn test_errors_when_test_session_id_missing() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        let event = test_discovery_session_event(
            "https://www.internalfb.com/intern/testinfra/testrun/12345",
            None,
        );
        let result = writer.handle_events(&[event]).await;

        assert!(result.is_err());
        assert!(!path.as_path().exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_errors_when_test_session_id_empty() -> buck2_error::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = AbsPathBuf::new(dir.path().join("test_id.txt"))?;
        let mut writer = TestIdWriter::new(path.clone());

        let event = test_discovery_session_event(
            "https://www.internalfb.com/intern/testinfra/testrun/12345",
            Some(""),
        );
        let result = writer.handle_events(&[event]).await;

        assert!(result.is_err());
        assert!(!path.as_path().exists());
        Ok(())
    }
}
