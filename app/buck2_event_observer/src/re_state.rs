/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::fmt;

use buck2_data::Snapshot;
use superconsole::DrawMode;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::style::Stylize;

use crate::humanized::HumanizedBytes;
use crate::humanized::HumanizedBytesPerSecond;
use crate::two_snapshots::TwoSnapshots;

pub struct ReState {
    pub session_id: Option<String>,
    first_snapshot: Option<buck2_data::Snapshot>,
}

/// Traffic the command has moved, measured against its first snapshot.
///
/// Downloads aggregate RE and HTTP; uploads are RE only.
pub struct NetworkStats {
    pub re_upload_bytes: u64,
    pub re_upload_bytes_per_second: u64,
    pub download_bytes: u64,
    pub download_bytes_per_second: u64,
}

pub struct ReHeaderLine<'a> {
    stats: Option<NetworkStats>,
    session_id: Option<&'a str>,
    draw_mode: DrawMode,
}

impl fmt::Display for ReHeaderLine<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Network:")?;
        let mut separator = " ";
        if let Some(stats) = &self.stats {
            match self.draw_mode {
                DrawMode::Normal => write!(
                    f,
                    "{separator}up {} {}  down {} {}",
                    HumanizedBytes::new(stats.re_upload_bytes),
                    BytesPerSecond(stats.re_upload_bytes_per_second),
                    HumanizedBytes::new(stats.download_bytes),
                    BytesPerSecond(stats.download_bytes_per_second),
                )?,
                DrawMode::Final => write!(
                    f,
                    "{separator}up {}  down {}",
                    HumanizedBytes::new(stats.re_upload_bytes),
                    HumanizedBytes::new(stats.download_bytes),
                )?,
            }
            separator = "  ";
        }
        if let Some(session_id) = self.session_id {
            write!(f, "{separator}session {session_id}")?;
        }
        Ok(())
    }
}

/// A transfer rate that renders as empty when zero, so idle directions stay blank
/// in the header instead of showing `0 B/s`.
struct BytesPerSecond(u64);

impl fmt::Display for BytesPerSecond {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == 0 {
            Ok(())
        } else {
            write!(f, "{}", HumanizedBytesPerSecond::new(self.0))
        }
    }
}

impl ReState {
    pub fn new() -> Self {
        Self {
            session_id: None,
            first_snapshot: None,
        }
    }

    pub fn add_re_session(&mut self, session: &buck2_data::RemoteExecutionSessionCreated) {
        self.session_id = Some(session.session_id.clone());
    }

    pub fn update(&mut self, snapshot: &buck2_data::Snapshot) {
        if self.first_snapshot.is_none() {
            self.first_snapshot = Some(snapshot.clone());
        }
    }

    pub fn first_snapshot(&self) -> &Option<Snapshot> {
        &self.first_snapshot
    }

    /// Returns [`None`] until snapshots exist, and for commands that have moved no bytes.
    pub fn network_stats(&self, two_snapshots: &TwoSnapshots) -> Option<NetworkStats> {
        let (Some(first), Some((_, last))) = (&self.first_snapshot, &two_snapshots.last) else {
            return None;
        };

        if last.re_upload_bytes == 0 && last.re_download_bytes == 0 && last.http_download_bytes == 0
        {
            return None;
        }

        let re_upload_bytes = last.re_upload_bytes.checked_sub(first.re_upload_bytes)?;
        let re_download_bytes = last
            .re_download_bytes
            .checked_sub(first.re_download_bytes)?;
        let http_download_bytes = last
            .http_download_bytes
            .checked_sub(first.http_download_bytes)?;

        Some(NetworkStats {
            re_upload_bytes,
            re_upload_bytes_per_second: two_snapshots
                .re_upload_bytes_per_second()
                .unwrap_or_default(),
            download_bytes: re_download_bytes + http_download_bytes,
            download_bytes_per_second: two_snapshots
                .re_download_bytes_per_second()
                .unwrap_or_default()
                + two_snapshots
                    .http_download_bytes_per_second()
                    .unwrap_or_default(),
        })
    }

    pub fn render_header(
        &self,
        two_snapshots: &TwoSnapshots,
        draw_mode: DrawMode,
    ) -> Option<ReHeaderLine<'_>> {
        let stats = self.network_stats(two_snapshots);
        let session_id = self.session_id.as_deref();
        if stats.is_none() && session_id.is_none() {
            return None;
        }
        Some(ReHeaderLine {
            stats,
            session_id,
            draw_mode,
        })
    }

    fn render_network_stat(
        &self,
        name: &str,
        bytes: u64,
        bytes_per_second: u64,
        draw_mode: DrawMode,
    ) -> buck2_error::Result<Line> {
        let mut stat = HumanizedBytes::new(bytes).to_string();
        if let DrawMode::Normal = draw_mode {
            if bytes_per_second > 0 {
                stat.push(' ');
                stat.push_str(&HumanizedBytesPerSecond::new(bytes_per_second).to_string());
            }
        }
        Ok(Line::unstyled(&format!("{name:<20}: {stat:>5}"))?)
    }

    fn render_detailed_items(
        &self,
        name: &str,
        started: u32,
        finished_successfully: u32,
        finished_with_error: u32,
    ) -> buck2_error::Result<Option<Line>> {
        let in_progress = started
            .saturating_sub(finished_successfully)
            .saturating_sub(finished_with_error);
        if in_progress == 0 && finished_successfully == 0 && finished_with_error == 0 {
            return Ok(None);
        }
        let line = format!(
            "{name:<20}: \
            {in_progress:>5} in progress, \
            {finished_successfully:>5} success, \
            {finished_with_error:>5} error"
        );
        Ok(Some(Line::unstyled(&line)?))
    }

    fn render_local_cache_stat(
        &self,
        name: &str,
        hits_files: i64,
        hits_bytes: i64,
        misses_files: i64,
        misses_bytes: i64,
    ) -> buck2_error::Result<Option<Line>> {
        let line = format!(
            "{:<20}: \
            {:>5} / {:>5} files hits, \
            {:>5} / {:>5} files misses ",
            name,
            HumanizedBytes::new(hits_bytes as u64),
            hits_files,
            HumanizedBytes::new(misses_bytes as u64),
            misses_files,
        );
        Ok(Some(Line::unstyled(&line)?))
    }

    fn render_detailed(
        &self,
        two_snapshots: &TwoSnapshots,
        draw_mode: DrawMode,
    ) -> buck2_error::Result<Vec<Line>> {
        let mut r = Vec::new();
        if self.session_id.is_some() || self.network_stats(two_snapshots).is_some() {
            r.push(Line::from_iter([Span::new_styled_lossy(
                "Network".to_owned().bold(),
            )]));
        }
        if let Some(session_id) = self.session_id.as_ref() {
            r.push(Line::unstyled(&format!("{:<20}: {session_id}", "session"))?);
        }
        if let Some(stats) = self.network_stats(two_snapshots) {
            r.push(self.render_network_stat(
                "up",
                stats.re_upload_bytes,
                stats.re_upload_bytes_per_second,
                draw_mode,
            )?);
            r.push(self.render_network_stat(
                "down",
                stats.download_bytes,
                stats.download_bytes_per_second,
                draw_mode,
            )?);
        }
        if let (Some(first), Some((_, last))) = (&self.first_snapshot, &two_snapshots.last) {
            r.extend(self.render_detailed_items(
                "re_uploads",
                last.re_uploads_started,
                last.re_uploads_finished_successfully,
                last.re_uploads_finished_with_error,
            )?);
            r.extend(self.render_detailed_items(
                "re_downloads",
                last.re_downloads_started,
                last.re_downloads_finished_successfully,
                last.re_downloads_finished_with_error,
            )?);
            r.extend(self.render_detailed_items(
                "re_action_cache",
                last.re_action_cache_started,
                last.re_action_cache_finished_successfully,
                last.re_action_cache_finished_with_error,
            )?);
            r.extend(self.render_detailed_items(
                "re_executes",
                last.re_executes_started,
                last.re_executes_finished_successfully,
                last.re_executes_finished_with_error,
            )?);
            r.extend(self.render_detailed_items(
                "re_materializes",
                last.re_materializes_started,
                last.re_materializes_finished_successfully,
                last.re_materializes_finished_with_error,
            )?);
            r.extend(self.render_detailed_items(
                "re_write_action_results",
                last.re_write_action_results_started,
                last.re_write_action_results_finished_successfully,
                last.re_write_action_results_finished_with_error,
            )?);
            r.extend(self.render_detailed_items(
                "re_get_digest_expirations",
                last.re_get_digest_expirations_started,
                last.re_get_digest_expirations_finished_successfully,
                last.re_get_digest_expirations_finished_with_error,
            )?);
            // TODO(raulgarcia4): Add some in-progress-stats for http metrics as well.
            let http_download_bytes = last.http_download_bytes - first.http_download_bytes;
            r.push(Line::unstyled(&format!(
                "{:<20}: \
                {:>5} bytes",
                "http_download_bytes", http_download_bytes
            ))?);

            r.extend(self.render_local_cache_stat(
                "local_artifact_cache",
                last.local_cache_hits_files - first.local_cache_hits_files,
                last.local_cache_hits_bytes - first.local_cache_hits_bytes,
                last.local_cache_misses_files - first.local_cache_misses_files,
                last.local_cache_misses_bytes - first.local_cache_misses_bytes,
            )?);
        }
        Ok(r)
    }

    pub fn render(
        &self,
        two_snapshots: &TwoSnapshots,
        detailed: bool,
        draw_mode: DrawMode,
    ) -> buck2_error::Result<Lines> {
        if detailed {
            return Ok(Lines(self.render_detailed(two_snapshots, draw_mode)?));
        }
        Ok(Lines::new())
    }
}
