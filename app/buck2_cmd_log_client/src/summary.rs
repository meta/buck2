/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::cmp::max;
use std::fmt::Display;
use std::fmt::Formatter;
use std::time::Duration;
use std::time::SystemTime;

use buck2_client_ctx::client_ctx::BuckSubcommand;
use buck2_client_ctx::client_ctx::ClientCommandContext;
use buck2_client_ctx::common::BuckArgMatches;
use buck2_client_ctx::event_log_options::EventLogOptions;
use buck2_client_ctx::events_ctx::EventsCtx;
use buck2_client_ctx::exit_result::ExitResult;
use buck2_client_ctx::subscribers::recorder::process_memory;
use buck2_data::ActionExecutionKind;
use buck2_event_log::stream_value::StreamValue;
use buck2_event_observer::fmt_duration;
use buck2_event_observer::humanized::HumanizedBytes;
use buck2_event_observer::humanized::HumanizedBytesPerSecond;
use buck2_util::network_speed_average::NetworkSpeedAverage;
use buck2_util::sliding_window::SlidingWindow;
use tokio_stream::StreamExt;

#[derive(Default)]
struct Stats {
    // TODO(yurysamkevich): add number of file changes since last build once availbale in log
    total_bytes_uploaded: u64,
    total_bytes_re_downloaded: u64,
    total_bytes_http_downloaded: u64,
    initial_re_download_bytes: Option<u64>,
    initial_http_download_bytes: Option<u64>,
    total_files_materialized: u64,
    total_bytes_materialized: u64,
    total_local_actions: u64,
    // TODO(yurysamkevich): split by RE platform - mac/windows/linux once available in log
    total_remote_actions: u64,
    total_cached_actions: u64,
    total_other_actions: u64,
    total_targets_analysed: u64,
    peak_process_memory_bytes: Option<u64>,
    re_avg_download_speed: NetworkSpeedAverage,
    re_avg_upload_speed: NetworkSpeedAverage,
    duration: Option<prost_types::Duration>,
    peak_used_disk_space_bytes: Option<u64>,
    total_disk_space_bytes: Option<u64>,
    system_total_memory_bytes: Option<u64>,
    re_max_download_speeds: Vec<SlidingWindow>,
    re_max_upload_speeds: Vec<SlidingWindow>,
    hg_revision: Option<String>,
    git_revision: Option<String>,
    has_local_changes: Option<bool>,
}

impl Stats {
    fn update_with_event(&mut self, event: &buck2_data::BuckEvent) {
        match &event.data {
            Some(buck2_data::buck_event::Data::SpanEnd(end)) => match end.data.as_ref() {
                Some(buck2_data::span_end_event::Data::ReUpload(data)) => {
                    self.total_bytes_uploaded += data.bytes_uploaded.unwrap_or_default();
                }
                Some(buck2_data::span_end_event::Data::Materialization(data)) => {
                    self.total_files_materialized += data.file_count;
                    self.total_bytes_materialized += data.total_bytes;
                }
                Some(buck2_data::span_end_event::Data::ActionExecution(data)) => {
                    match ActionExecutionKind::try_from(data.execution_kind) {
                        Ok(ActionExecutionKind::Local) => self.total_local_actions += 1,
                        Ok(ActionExecutionKind::Remote)
                        | Ok(ActionExecutionKind::RemoteDeduplicated) => {
                            self.total_remote_actions += 1
                        }
                        Ok(ActionExecutionKind::ActionCache) => self.total_cached_actions += 1,
                        _ => self.total_other_actions += 1,
                    }
                }
                Some(buck2_data::span_end_event::Data::Analysis(_)) => {
                    self.total_targets_analysed += 1;
                }
                Some(buck2_data::span_end_event::Data::Command(_command)) => {
                    self.duration = end.duration;
                }
                _ => {}
            },
            Some(buck2_data::buck_event::Data::Instant(instant_event)) => {
                match instant_event.data.as_ref() {
                    Some(buck2_data::instant_event::Data::Snapshot(snapshot)) => {
                        self.peak_process_memory_bytes =
                            max(self.peak_process_memory_bytes, process_memory(snapshot));
                        self.peak_used_disk_space_bytes = max(
                            self.peak_used_disk_space_bytes,
                            snapshot.used_disk_space_bytes,
                        );
                        // Download counters are cumulative across invocations of the daemon.
                        self.total_bytes_re_downloaded = snapshot.re_download_bytes.saturating_sub(
                            *self
                                .initial_re_download_bytes
                                .get_or_insert(snapshot.re_download_bytes),
                        );
                        self.total_bytes_http_downloaded =
                            snapshot.http_download_bytes.saturating_sub(
                                *self
                                    .initial_http_download_bytes
                                    .get_or_insert(snapshot.http_download_bytes),
                            );

                        if let Some(ts) = get_event_timestamp(event) {
                            self.re_avg_download_speed
                                .update(ts, snapshot.re_download_bytes);
                            self.re_avg_upload_speed
                                .update(ts, snapshot.re_upload_bytes);

                            for s in self.re_max_download_speeds.iter_mut() {
                                s.update(ts, snapshot.re_download_bytes);
                            }

                            for s in self.re_max_upload_speeds.iter_mut() {
                                s.update(ts, snapshot.re_upload_bytes);
                            }
                        }
                    }
                    Some(buck2_data::instant_event::Data::SystemInfo(system_info)) => {
                        self.total_disk_space_bytes = system_info.total_disk_space_bytes;
                        self.system_total_memory_bytes = system_info.system_total_memory_bytes;
                    }
                    Some(buck2_data::instant_event::Data::VersionControlRevision(vcs)) => {
                        if let Some(ref revision) = vcs.hg_revision {
                            self.hg_revision = Some(revision.clone());
                        }
                        if let Some(ref revision) = vcs.git_revision {
                            self.git_revision = Some(revision.clone());
                        }
                        if let Some(ref has_local_changes) = vcs.has_local_changes {
                            self.has_local_changes = Some(*has_local_changes);
                        }
                    }
                    _ => {}
                }
            }

            _ => {}
        }
    }
}

fn get_event_timestamp(event: &buck2_data::BuckEvent) -> Option<SystemTime> {
    SystemTime::try_from(event.timestamp?).ok()
}

impl Display for Stats {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // Build Summary section
        writeln!(f)?;
        writeln!(f, "Build Summary")?;

        if let Some(duration) = &self.duration {
            let duration = std::time::Duration::new(duration.seconds as u64, duration.nanos as u32);
            writeln!(f, "- Duration: {}", fmt_duration::fmt_duration(duration))?;
        }

        writeln!(f, "- Targets Analyzed: {}", self.total_targets_analysed)?;

        if let Some(hg_revision) = &self.hg_revision {
            writeln!(f, "- HG Revision: {hg_revision}")?;
        }

        if let Some(git_revision) = &self.git_revision {
            writeln!(f, "- Git Revision: {git_revision}")?;
        }

        if let Some(has_local_changes) = self.has_local_changes {
            writeln!(f, "- Has Local Changes: {has_local_changes}")?;
        } else {
            writeln!(f, "- Has Local Changes: unknown")?;
        }

        writeln!(f)?;

        // Actions section
        writeln!(f, "Actions")?;
        writeln!(f, "- Local actions: {}", self.total_local_actions)?;
        writeln!(f, "- Remote actions: {}", self.total_remote_actions)?;
        writeln!(f, "- Cached actions: {}", self.total_cached_actions)?;
        writeln!(f, "- Other actions: {}", self.total_other_actions)?;
        writeln!(f)?;

        // File & Data section
        writeln!(f, "File & Data")?;
        writeln!(
            f,
            "  (Network transfer uses compression, file materialized shows uncompressed sizes)"
        )?;
        writeln!(f, "- Files materialized: {}", self.total_files_materialized)?;
        writeln!(
            f,
            "- Total materialized: {}",
            HumanizedBytes::new(self.total_bytes_materialized)
        )?;
        writeln!(
            f,
            "- Total uploaded: {}",
            HumanizedBytes::new(self.total_bytes_uploaded)
        )?;
        writeln!(
            f,
            "- Total downloaded: {}",
            HumanizedBytes::new(self.total_bytes_re_downloaded + self.total_bytes_http_downloaded)
        )?;
        writeln!(
            f,
            "  - RE downloaded: {}",
            HumanizedBytes::new(self.total_bytes_re_downloaded)
        )?;
        writeln!(
            f,
            "  - HTTP downloaded: {}",
            HumanizedBytes::new(self.total_bytes_http_downloaded)
        )?;
        writeln!(f)?;

        // Network section
        writeln!(f, "Network")?;

        // Download Speed grouping
        let has_download_data = self.re_avg_download_speed.avg_per_second().is_some()
            || self
                .re_max_download_speeds
                .iter()
                .any(|w| w.max_per_second().unwrap_or_default() > 0);

        if has_download_data {
            writeln!(f, "- Download Speed")?;
            if let Some(re_avg_download_speed) = self.re_avg_download_speed.avg_per_second() {
                writeln!(
                    f,
                    "  - Average: {}",
                    HumanizedBytesPerSecond::fixed_width(re_avg_download_speed)
                )?;
            }

            if let Some(re_max_download_speed) = self
                .re_max_download_speeds
                .iter()
                .map(|w| w.max_per_second().unwrap_or_default())
                .max()
            {
                writeln!(
                    f,
                    "  - Peak: {}",
                    HumanizedBytesPerSecond::fixed_width(re_max_download_speed)
                )?;
            }
        }

        // Upload Speed grouping
        let has_upload_data = self.re_avg_upload_speed.avg_per_second().is_some()
            || self
                .re_max_upload_speeds
                .iter()
                .any(|w| w.max_per_second().unwrap_or_default() > 0);

        if has_upload_data {
            writeln!(f, "- Upload Speed")?;
            if let Some(re_avg_upload_speed) = self.re_avg_upload_speed.avg_per_second() {
                writeln!(
                    f,
                    "  - Average: {}",
                    HumanizedBytesPerSecond::fixed_width(re_avg_upload_speed)
                )?;
            }

            if let Some(re_max_upload_speed) = self
                .re_max_upload_speeds
                .iter()
                .map(|w| w.max_per_second().unwrap_or_default())
                .max()
            {
                writeln!(
                    f,
                    "  - Peak: {}",
                    HumanizedBytesPerSecond::fixed_width(re_max_upload_speed)
                )?;
            }
        }
        writeln!(f)?;

        // Resource Usage section
        writeln!(f, "Resource Usage")?;
        if let (Some(peak_process_memory_bytes), Some(system_total_memory_bytes)) = (
            self.peak_process_memory_bytes,
            self.system_total_memory_bytes,
        ) {
            writeln!(
                f,
                "- Peak process memory: {} / {}",
                HumanizedBytes::fixed_width(peak_process_memory_bytes),
                HumanizedBytes::fixed_width(system_total_memory_bytes)
            )?;
        }
        if let (Some(peak_used_disk_space_bytes), Some(total_disk_space_bytes)) =
            (self.peak_used_disk_space_bytes, self.total_disk_space_bytes)
        {
            writeln!(
                f,
                "- Peak used disk space: {} / {}",
                HumanizedBytes::fixed_width(peak_used_disk_space_bytes),
                HumanizedBytes::fixed_width(total_disk_space_bytes)
            )?;
        }

        Ok(())
    }
}

/// Outputs high level statistics about the build
#[derive(Debug, clap::Parser)]
pub struct SummaryCommand {
    #[clap(flatten)]
    event_log: EventLogOptions,
}

impl BuckSubcommand for SummaryCommand {
    const COMMAND_NAME: &'static str = "log-summary";

    async fn exec_impl(
        self,
        _matches: BuckArgMatches<'_>,
        ctx: ClientCommandContext<'_>,
        _events_ctx: &mut EventsCtx,
    ) -> ExitResult {
        let log_path = self.event_log.get(&ctx).await?;

        let (invocation, mut events) = log_path.unpack_stream().await?;

        buck2_client_ctx::println!(
            "Showing summary from: {}",
            invocation.display_command_line()
        )?;
        buck2_client_ctx::println!("build ID: {}", invocation.trace_id)?;

        let mut stats = Stats {
            re_max_download_speeds: vec![
                SlidingWindow::new(Duration::from_secs(1)),
                SlidingWindow::new(Duration::from_secs(5)),
                SlidingWindow::new(Duration::from_secs(10)),
            ],
            re_max_upload_speeds: vec![
                SlidingWindow::new(Duration::from_secs(1)),
                SlidingWindow::new(Duration::from_secs(5)),
                SlidingWindow::new(Duration::from_secs(10)),
            ],
            ..Default::default()
        };

        while let Some(event) = events.try_next().await? {
            match event {
                StreamValue::Event(event) => stats.update_with_event(&event),
                StreamValue::Result(..) | StreamValue::PartialResult(..) => {}
            }
        }
        buck2_client_ctx::println!("{}", stats)?;
        ExitResult::success()
    }
}
