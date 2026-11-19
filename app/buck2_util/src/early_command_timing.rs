/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Timing for the phases of command startup that run before the build proper begins.

use std::time::Instant;

pub const OTHER_COMMAND_START_OVERHEAD: &str = "other-command-start-overhead";
pub const EXCLUSIVE_COMMAND_WAIT: &str = "exclusive-command-wait";
pub const FILE_WATCHER_WAIT: &str = "file-watcher-wait";

pub struct EarlyCommandTiming {
    pub command_start: Instant,
    pub early_spans: Vec<(Instant, String)>,
    pub early_command_end: Instant,
}

pub struct EarlyCommandTimingBuilder {
    command_start: Instant,
    early_spans: Vec<(Instant, String)>,
}

impl EarlyCommandTimingBuilder {
    pub fn new(command_start: Instant) -> Self {
        Self {
            command_start,
            early_spans: Vec::new(),
        }
    }

    pub fn start_span(&mut self, name: String) {
        self.early_spans.push((Instant::now(), name));
    }

    pub fn end_known_span(&mut self) {
        self.early_spans
            .push((Instant::now(), OTHER_COMMAND_START_OVERHEAD.to_owned()));
    }

    pub fn finish_early_command_timing(self) -> EarlyCommandTiming {
        EarlyCommandTiming {
            command_start: self.command_start,
            early_spans: self.early_spans,
            early_command_end: Instant::now(),
        }
    }
}
