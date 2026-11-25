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
use std::sync::atomic::AtomicU64;
use std::time::SystemTime;

use async_trait::async_trait;
use buck2_common::argv::SanitizedArgv;
use buck2_core::soft_error;
use buck2_event_log::FutureChildOutput;
use buck2_event_log::wait_for_child_and_log;
use buck2_event_log::write::EventLogFiles;
use buck2_event_log::write::WriteEventLog;
use buck2_event_log::write::start_event_log_uploader;
use buck2_events::BuckEvent;
use buck2_fs::working_dir::AbsWorkingDir;

use crate::subscribers::subscriber::EventSubscriber;
use crate::ticker::Tick;

/// Writes the events of a command to its event log, and to the copies of the log that the
/// command's arguments ask for, and has the log uploaded while it is written.
///
/// A command whose log cannot be written goes on without the rest of it. One that cannot
/// write a copy the user asked for fails.
///
/// The log is opened before the copies: opening it removes the old logs of its directory, and
/// that is every file there, so a copy in that directory has to be created after. The copies
/// are ended before the log, which waits for its upload when it is ended.
pub(crate) struct EventLog {
    /// `None` if the daemon writes the log, and once the log could not be written.
    log: Option<WriteEventLog>,
    copies: Option<WriteEventLog>,
    /// The process that uploads the log, from when the log is opened.
    uploader: Option<FutureChildOutput>,
}

impl EventLog {
    /// `log` is the command's event log, unless the daemon writes it, and `copies` its
    /// copies, if any.
    pub(crate) fn new(
        log: Option<EventLogFiles>,
        copies: Option<EventLogFiles>,
        working_dir: AbsWorkingDir,
        sanitized_argv: SanitizedArgv,
        start_time: SystemTime,
        log_size_counter_bytes: Option<Arc<AtomicU64>>,
    ) -> EventLog {
        // Few commands have copies: only then is anything cloned.
        let copies = copies.map(|files| {
            WriteEventLog::new(
                files,
                working_dir.clone(),
                sanitized_argv.clone(),
                start_time,
                log_size_counter_bytes.clone(),
            )
        });
        let log = log.map(|files| {
            WriteEventLog::new(
                files,
                working_dir,
                sanitized_argv,
                start_time,
                log_size_counter_bytes,
            )
        });
        Self {
            log,
            copies,
            uploader: None,
        }
    }

    /// Takes the outcome of writing to the log. A failure ends the log and is reported.
    fn log_written(&mut self, res: buck2_error::Result<()>) -> buck2_error::Result<()> {
        if let Err(e) = res {
            // This frees the log's writer lock, so the uploader uploads what the file holds
            // and exits. A log without its end is uploaded on purpose: what was written is
            // worth having, and the soft error says that the rest is missing.
            self.log = None;
            soft_error!("event_log_write_failed", e);
        }
        Ok(())
    }
}

#[async_trait]
impl EventSubscriber for EventLog {
    fn name(&self) -> &'static str {
        "event log"
    }

    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        if let Some(log) = &mut self.log {
            let was_open = log.command_log().is_some();
            let res = log.write_events(events).await;
            if !was_open
                && let Some(path) = log.command_log()
                && let Some(event) = events.first()
            {
                // Not being able to start the uploader fails the command, where not being
                // able to write the log does not.
                self.uploader = start_event_log_uploader(path, &event.trace_id()?)?;
            }
            self.log_written(res)?;
        }
        if let Some(copies) = &mut self.copies {
            copies.write_events(events).await?;
        }
        Ok(())
    }

    async fn handle_tailer_stderr(&mut self, _stderr: &str) -> buck2_error::Result<()> {
        // TODO(nga): currently we mostly ignore buckd stderr.
        //   It is very important to investigate crashes of buckd.
        //
        //   We attach truncated log to Scuba since D53337966
        //   (although we probably shouldn't do that).
        //
        //   Regardless of that we should do either or both of the following:
        //   - write it to event log if it is interesting (e.g. crash)
        //   - upload it to manifold unconditionally as a separate file
        //     (but only relevant part, since command start)
        Ok(())
    }

    async fn handle_command_result(
        &mut self,
        result: &buck2_cli_proto::CommandResult,
    ) -> buck2_error::Result<()> {
        if let Some(log) = &mut self.log {
            let res = log.write_result(result).await;
            self.log_written(res)?;
        }
        if let Some(copies) = &mut self.copies {
            copies.write_result(result).await?;
        }
        Ok(())
    }

    /// Flush all log files during on tick to avoid buffering data in memory which we might lose if
    /// we hit an error.
    async fn tick(&mut self, _tick: &Tick) -> buck2_error::Result<()> {
        if let Some(log) = &mut self.log {
            let res = log.flush_files().await;
            self.log_written(res)?;
        }
        if let Some(copies) = &mut self.copies {
            copies.flush_files().await?;
        }
        Ok(())
    }

    async fn finalize(mut self: Box<Self>) -> buck2_error::Result<()> {
        if let Some(copies) = &mut self.copies {
            copies.exit().await;
        }
        if let Some(log) = &mut self.log {
            log.exit().await;
        }
        if let Some(uploader) = self.uploader {
            wait_for_child_and_log(uploader, "Event Log").await;
        }
        Ok(())
    }
}
