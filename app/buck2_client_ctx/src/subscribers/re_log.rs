/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use buck2_event_log::FutureChildOutput;
use buck2_event_log::should_block_on_log_upload;
use buck2_event_log::should_upload_log;
use buck2_event_log::wait_for_child_and_log;
use buck2_event_observer::unpack_event::UnpackedBuckEvent;
use buck2_event_observer::unpack_event::unpack_event;
use buck2_events::BuckEvent;
use buck2_fs::paths::file_name::FileNameBuf;
use futures::Future;

use crate::subscribers::subscriber::EventSubscriber;

/// An RE session whose client logs are yet to be uploaded.
struct ReSession {
    id: String,
    logs_dir: String,
}

pub(crate) struct ReLog {
    re_session: Option<ReSession>,
    isolation_dir: FileNameBuf,
}

impl ReLog {
    pub(crate) fn new(isolation_dir: FileNameBuf) -> Self {
        Self {
            re_session: None,
            isolation_dir,
        }
    }

    fn log_upload(
        &mut self,
    ) -> impl Future<Output = buck2_error::Result<()>> + 'static + Send + Sync + use<> {
        // We put `None` in place of re_session which means we will only attempt to upload
        // the logs once no matter how many times this function is called
        let session = self.re_session.take();
        let isolation_dir = self.isolation_dir.clone();
        async move {
            if let Some(session) = session {
                log_upload_impl(session, isolation_dir).await?;
            }
            Ok(())
        }
    }
}

#[async_trait]
impl EventSubscriber for ReLog {
    fn name(&self) -> &'static str {
        "RE log"
    }

    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        for event in events {
            if let UnpackedBuckEvent::Instant(
                _,
                _,
                buck2_data::instant_event::Data::ReSession(session),
            ) = unpack_event(event)?
            {
                self.re_session = session.logs_dir.as_ref().map(|logs_dir| ReSession {
                    id: session.session_id.clone(),
                    logs_dir: logs_dir.clone(),
                });
            }
        }
        Ok(())
    }

    async fn finalize(mut self: Box<Self>) -> buck2_error::Result<()> {
        self.log_upload().await
    }
}

async fn log_upload_impl(
    session: ReSession,
    isolation_dir: FileNameBuf,
) -> buck2_error::Result<()> {
    if !should_upload_log()? {
        return Ok(());
    }

    let mut buck = buck2_util::process::async_background_command(std::env::current_exe()?);
    let command = buck
        .arg("--isolation-dir")
        .arg(isolation_dir.as_str())
        .arg("debug")
        .arg("upload-re-logs")
        .arg("--session-id")
        .arg(session.id)
        .arg("--logs-dir")
        .arg(session.logs_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    if should_block_on_log_upload()? {
        let child = command.stderr(Stdio::piped()).spawn()?;
        wait_for_child_and_log(FutureChildOutput::new(child), "RE Log").await;
    } else {
        command.stderr(Stdio::null()).spawn()?;
    };

    Ok(())
}
