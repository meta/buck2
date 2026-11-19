/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use allocative::Allocative;
use buck2_core::fs::i_project_rel_path::IProjectRelativePath;

/// We limit the number of file change records so we don't use too much memory
/// or too much space in scribe.
///
/// 100 entries covers everything required for 96% of updates, which seems sufficient.
/// Number needs to be < 850 or it is often bigger than a scribe message.
const MAX_FILE_CHANGE_RECORDS: usize = 100;

#[derive(Allocative)]
pub(crate) struct FileWatcherStats {
    stats: buck2_data::FileWatcherStats,
    // Bounded by MAX_FILE_CHANGE_RECORDS
    changes: Vec<buck2_data::FileWatcherEvent>,
    // Did we not insert things into changes
    changes_missed: bool,
}

impl FileWatcherStats {
    pub(crate) fn new(stats: buck2_data::FileWatcherStats, min_count: usize) -> Self {
        let changes = Vec::with_capacity(std::cmp::min(MAX_FILE_CHANGE_RECORDS, min_count));

        Self {
            stats,
            changes,
            changes_missed: false,
        }
    }

    /// I have seen an event that I am ignoring
    pub(crate) fn add_ignored(&mut self, count: u64) {
        self.stats.events_total += count;
    }

    /// I have seen an event that I am processing
    pub(crate) fn add(
        &mut self,
        path: IProjectRelativePath,
        event: buck2_data::FileWatcherEventType,
        kind: buck2_data::FileWatcherKind,
    ) {
        self.stats.events_total += 1;
        self.stats.events_processed += 1;

        if self.changes.len() < MAX_FILE_CHANGE_RECORDS {
            self.changes.push(buck2_data::FileWatcherEvent {
                event: event as i32,
                kind: kind as i32,
                path: path.to_string(),
            });
        } else {
            self.changes_missed = true;
        }
    }

    pub(crate) fn finish(self) -> buck2_data::FileWatcherStats {
        let Self {
            mut stats,
            changes,
            changes_missed,
        } = self;

        stats.events = changes;
        if changes_missed {
            let reason = format!(
                "Too many files changed ({}, max {})",
                stats.events_processed, MAX_FILE_CHANGE_RECORDS
            );
            stats.incomplete_events_reason = Some(reason);
        }

        stats
    }
}
