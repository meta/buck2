/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_health_check::report::DisplayReport;
use buck2_health_check::report::Message;
use superconsole::Component;
use superconsole::Dimensions;
use superconsole::DrawMode;
use superconsole::Lines;

use crate::subscribers::superconsole::message_renderer::render_rich_message_lines;
use crate::subscribers::superconsole::message_renderer::render_rich_message_lines_compact;
use crate::subscribers::superconsole::message_renderer::warning_styled;
use crate::subscribers::system_warning::check_memory_pressure_snapshot;
use crate::subscribers::system_warning::check_remaining_disk_space_snapshot;
use crate::subscribers::system_warning::low_disk_space_msg;
use crate::subscribers::system_warning::system_memory_exceeded_msg;

/// This component is used to display system warnings for a command e.g. memory pressure, low disk space etc.
pub(crate) struct SystemWarningComponent<'a> {
    pub(crate) last_snapshot: Option<&'a buck2_data::Snapshot>,
    pub(crate) system_info: &'a buck2_data::SystemInfo,
    pub(crate) health_check_reports: Option<&'a Vec<DisplayReport>>,
}

impl Component for SystemWarningComponent<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        _dimensions: Dimensions,
        _mode: DrawMode,
    ) -> buck2_error::Result<Lines> {
        let mut lines = Vec::new();

        if let Some(memory_pressure) =
            check_memory_pressure_snapshot(self.last_snapshot, self.system_info)
        {
            lines.push(warning_styled(&system_memory_exceeded_msg(
                &memory_pressure,
            ))?);
        }
        if let Some(low_disk_space) =
            check_remaining_disk_space_snapshot(self.last_snapshot, self.system_info)
        {
            lines.push(warning_styled(&low_disk_space_msg(&low_disk_space))?);
        }
        if let Some(reports) = self.health_check_reports {
            for report in reports {
                if let Some(warning) = &report.health_issue {
                    match &warning.message {
                        Message::Simple(text) => {
                            lines.push(warning_styled(text)?);
                        }
                        Message::Rich {
                            header,
                            body,
                            footer,
                            compact,
                        } => {
                            let rich_lines = if *compact {
                                render_rich_message_lines_compact(header, body, footer.as_deref())?
                            } else {
                                render_rich_message_lines(header, body, footer.as_deref())?
                            };
                            lines.extend(rich_lines);
                        }
                    }
                }
            }
        }
        Ok(Lines(lines))
    }
}
