/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::iter;

use buck2_event_observer::humanized::HumanizedBytes;
use buck2_event_observer::humanized::HumanizedBytesPerSecond;
use buck2_event_observer::re_state::NetworkStats;
use buck2_event_observer::re_state::ReState;
use buck2_event_observer::session_info::SessionInfo;
use buck2_event_observer::two_snapshots::TwoSnapshots;
use superconsole::Component;
use superconsole::Dimensions;
use superconsole::DrawMode;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;

/// This component is used to display session information for a command e.g. RE session ID
pub struct SessionInfoComponent<'s> {
    pub session_info: &'s SessionInfo,
    pub re_state: &'s ReState,
    pub two_snapshots: &'s TwoSnapshots,
}

impl Component for SessionInfoComponent<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(&self, dimensions: Dimensions, mode: DrawMode) -> buck2_error::Result<Lines> {
        let mut headers = Lines::new();
        let mut ids = vec![];
        if let Some(invocation_url) = self.session_info.invocation_url() {
            headers.push(Line::unstyled("Build URL:")?);
            ids.push(Span::new_unstyled(invocation_url)?);
        }
        if cfg!(fbcode_build) {
            headers.push(Line::unstyled("Buck UI:")?);
            ids.push(Span::new_unstyled(format!(
                "https://www.internalfb.com/buck2/{}",
                self.session_info.trace_id
            ))?);
        } else {
            headers.push(Line::unstyled("Build ID:")?);
            ids.push(Span::new_unstyled(&self.session_info.trace_id)?);
        }
        if let Some(buck2_data::TestSessionInfo { info, .. }) = &self.session_info.test_session {
            headers.push(Line::unstyled("Test UI:")?);
            ids.push(Span::new_unstyled(info)?);
        }
        if let Some(NetworkStats {
            re_upload_bytes,
            re_upload_bytes_per_second,
            download_bytes,
            download_bytes_per_second,
        }) = self.re_state.network_stats(self.two_snapshots)
        {
            match mode {
                DrawMode::Normal => {
                    let rate = |bytes_per_second: u64| {
                        if bytes_per_second == 0 {
                            " ".repeat(HumanizedBytesPerSecond::FIXED_WIDTH_WIDTH)
                        } else {
                            HumanizedBytesPerSecond::fixed_width(bytes_per_second).to_string()
                        }
                    };
                    headers.push(Line::unstyled("Network:")?);
                    ids.push(Span::new_unstyled(format!(
                        "{:<4} {} {}",
                        "up",
                        HumanizedBytes::fixed_width(re_upload_bytes),
                        rate(re_upload_bytes_per_second),
                    ))?);
                    headers.push(Line::default());
                    ids.push(Span::new_unstyled(format!(
                        "{:<4} {} {}",
                        "down",
                        HumanizedBytes::fixed_width(download_bytes),
                        rate(download_bytes_per_second),
                    ))?);
                }
                DrawMode::Final => {
                    headers.push(Line::unstyled("Network:")?);
                    ids.push(Span::new_unstyled(format!(
                        "up {}  down {}",
                        HumanizedBytes::new(re_upload_bytes),
                        HumanizedBytes::new(download_bytes),
                    ))?);
                }
            }
        }
        if self.session_info.legacy_dice {
            headers.push(Line::unstyled("Note:")?);
            ids.push(Span::new_unstyled(
                "Using deprecated legacy dice".to_owned(),
            )?);
        }
        // pad all headers to the max width.
        headers.justify();
        headers.pad_lines_right(1);

        let max_len = headers
            .iter()
            .zip(ids.iter())
            .map(|(header, id)| header.len() + id.len())
            .max()
            .unwrap_or(0);

        let lines = if max_len > dimensions.width {
            headers
                .into_iter()
                .zip(ids)
                .flat_map(|(header, id)| {
                    iter::once(header).chain(iter::once(Line::from_iter([id])))
                })
                .collect()
        } else {
            headers
                .iter_mut()
                .zip(ids)
                .for_each(|(header, id)| header.push(id));
            headers
        };

        let max_len = lines.iter().map(|line| line.len()).max().unwrap_or(0);

        Ok(if max_len > dimensions.width {
            Lines(vec![Line::unstyled(
                "<Terminal too small for build details>",
            )?])
        } else {
            lines
        })
    }
}
