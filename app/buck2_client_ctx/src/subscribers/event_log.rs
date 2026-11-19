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
use buck2_event_log::write::WriteEventLog;
use buck2_events::BuckEvent;
use buck2_fs::paths::abs_norm_path::AbsNormPathBuf;
use buck2_fs::paths::abs_path::AbsPathBuf;
use buck2_fs::working_dir::AbsWorkingDir;

use crate::subscribers::subscriber::EventSubscriber;
use crate::ticker::Tick;

/// This EventLog lets us to events emitted by Buck and log them to a file. The events are
/// serialized as JSON and logged one per line.
pub(crate) struct EventLog {
    writer: WriteEventLog,
}

impl EventLog {
    pub(crate) fn new(
        logdir: AbsNormPathBuf,
        working_dir: AbsWorkingDir,
        extra_path: Option<AbsPathBuf>,
        extra_user_event_log_path: Option<AbsPathBuf>,
        sanitized_argv: SanitizedArgv,
        command_name: String,
        start_time: SystemTime,
        log_size_counter_bytes: Option<Arc<AtomicU64>>,
        retained_event_logs: usize,
    ) -> EventLog {
        Self {
            writer: WriteEventLog::new(
                logdir,
                working_dir,
                extra_path,
                extra_user_event_log_path,
                sanitized_argv,
                command_name,
                start_time,
                log_size_counter_bytes,
                retained_event_logs,
            ),
        }
    }
}

#[async_trait]
impl EventSubscriber for EventLog {
    fn name(&self) -> &'static str {
        "event log"
    }

    async fn handle_events(&mut self, events: &[Arc<BuckEvent>]) -> buck2_error::Result<()> {
        Ok(self.writer.write_events(events).await?)
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
        Ok(self.writer.write_result(result).await?)
    }

    /// Flush all log files during on tick to avoid buffering data in memory which we might lose if
    /// we hit an error.
    async fn tick(&mut self, _tick: &Tick) -> buck2_error::Result<()> {
        Ok(self.writer.flush_files().await?)
    }

    async fn finalize(mut self: Box<Self>) -> buck2_error::Result<()> {
        self.writer.exit().await;
        Ok(())
    }
}
