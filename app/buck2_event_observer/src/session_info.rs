/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_wrapper_common::invocation_id::TraceId;

/// Information about the current command, such as session or build ids.
pub struct SessionInfo {
    pub trace_id: TraceId,
    pub bes_results_url: Option<String>,
    pub test_session: Option<buck2_data::TestSessionInfo>,
    pub legacy_dice: bool,
}

impl SessionInfo {
    pub fn invocation_url(&self) -> Option<String> {
        let base_url = self
            .bes_results_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?;
        let trace_id = self.trace_id.to_string();

        if base_url.contains("%s") {
            return Some(base_url.replace("%s", &trace_id));
        }
        if base_url.ends_with('/') || base_url.ends_with('=') {
            return Some(format!("{base_url}{trace_id}"));
        }
        Some(format!("{base_url}/{trace_id}"))
    }

    pub fn streaming_results_line(&self) -> Option<String> {
        Some(format!(
            "Streaming build results to: {}",
            self.invocation_url()?
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    fn session_info_with_url(base_url: &str) -> SessionInfo {
        SessionInfo {
            trace_id: TraceId::from_str("00000000-0000-4000-8000-000000000123").unwrap(),
            bes_results_url: Some(base_url.to_owned()),
            test_session: None,
            legacy_dice: false,
        }
    }

    #[test]
    fn invocation_url_appends_trace_id() {
        let info = session_info_with_url("https://buildbuddy.example.com/invocation");
        assert_eq!(
            info.invocation_url().as_deref(),
            Some("https://buildbuddy.example.com/invocation/00000000-0000-4000-8000-000000000123")
        );
    }

    #[test]
    fn invocation_url_supports_percent_s_placeholder() {
        let info = session_info_with_url("https://buildbuddy.example.com/invocation/%s");
        assert_eq!(
            info.invocation_url().as_deref(),
            Some("https://buildbuddy.example.com/invocation/00000000-0000-4000-8000-000000000123")
        );
    }

    #[test]
    fn streaming_results_line_uses_invocation_url() {
        let info = session_info_with_url("https://buildbuddy.example.com/invocation/");
        assert_eq!(
            info.streaming_results_line().as_deref(),
            Some(
                "Streaming build results to: https://buildbuddy.example.com/invocation/00000000-0000-4000-8000-000000000123"
            )
        );
    }

    #[test]
    fn streaming_results_line_is_absent_without_invocation_url() {
        let info = SessionInfo {
            trace_id: TraceId::from_str("00000000-0000-4000-8000-000000000123").unwrap(),
            bes_results_url: None,
            test_session: None,
            legacy_dice: false,
        };
        assert_eq!(info.streaming_results_line().as_deref(), None);
    }
}
