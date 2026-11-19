/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_event_observer::debug_events::DebugEventsState;
use superconsole::Component;
use superconsole::Lines;

use crate::subscribers::superconsole::SuperConsoleConfig;

pub(crate) struct DebugEventsComponent<'s> {
    pub(crate) super_console_config: &'s SuperConsoleConfig,
    pub(crate) debug_events_state: &'s DebugEventsState,
}

impl Component for DebugEventsComponent<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        _dimensions: superconsole::Dimensions,
        _mode: superconsole::DrawMode,
    ) -> buck2_error::Result<superconsole::Lines> {
        if !self.super_console_config.enable_debug_events {
            return Ok(Lines::new());
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push(format!(
            "Events...  total: {} maximum delay: {:.3}ms average delay last {}: {:.3}ms",
            self.debug_events_state.event_count,
            self.debug_events_state.max_delay.as_secs_f64() * 1000.0,
            self.debug_events_state.recent_delays.len(),
            if self.debug_events_state.recent_delays.is_empty() {
                0.0
            } else {
                self.debug_events_state
                    .recent_delays
                    .iter()
                    .sum::<Duration>()
                    .as_secs_f64()
                    / (self.debug_events_state.recent_delays.len() as f64)
                    * 1000.0
            }
        ));

        if !self.debug_events_state.spans.is_empty() {
            let header_line = format!(
                "  {:<32}  {:>10}  {:>10}  {:>9}  {:>13}  {:>14}",
                "Span Events", "started", "finished", "duration", "poll time", "avg max poll time"
            );
            let header_len = header_line.len();
            lines.push(header_line);
            lines.push("-".repeat(header_len));
            for (k, v) in self.debug_events_state.spans.iter() {
                lines.push(format!(
                    "    {:<30} |{:>10} |{:>10} |{:8.3}s |{:>12.3}s |{:>12}us",
                    k,
                    v.started,
                    v.finished,
                    v.total_duration.as_secs_f64(),
                    v.total_poll_time.as_secs_f64(),
                    if v.finished == 0 {
                        0
                    } else {
                        v.total_max_poll_time.as_micros() / (v.finished as u128)
                    },
                ));
            }
            lines.push("-".repeat(header_len));
        }

        if !self.debug_events_state.instants.is_empty() {
            let header_line = format!("  {:<32}  {:>12}", "Instant Events", "count");
            let header_len = header_line.len();
            lines.push(header_line);
            lines.push("-".repeat(header_len));
            for (k, v) in self.debug_events_state.instants.iter() {
                lines.push(format!("    {:<30} |{:>12}", k, v.count));
            }
            lines.push("-".repeat(header_len));
        }

        Ok(Lines(
            lines
                .into_iter()
                .map(|v| vec![v].try_into())
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }
}
