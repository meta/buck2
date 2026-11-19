/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_error::BuckErrorOptionContext;
use buck2_event_observer::test_state::TestState;
use crossterm::style::Color;
use crossterm::style::ContentStyle;
use superconsole::Component;
use superconsole::Dimensions;
use superconsole::DrawMode;
use superconsole::Line;
use superconsole::Lines;
use superconsole::Span;
use superconsole::SpanError;

use crate::subscribers::superconsole::SessionInfo;

struct TestCounterComponent;

pub struct TestCounterColumn {
    label: &'static str,
    color: Option<Color>,
    get_from_test_state: fn(&TestState) -> u64,
    get_from_test_statues: fn(
        &buck2_cli_proto::test_response::TestStatuses,
    ) -> &Option<buck2_cli_proto::CounterWithExamples>,
}

impl TestCounterColumn {
    pub const LISTING_FAIL: TestCounterColumn = TestCounterColumn {
        label: "Listing Fail",
        color: Some(Color::Red),
        get_from_test_state: |test_state| test_state.listing_failed,
        get_from_test_statues: |test_statuses| &test_statuses.listing_failed,
    };
    const DISCOVERED: TestCounterColumn = TestCounterColumn {
        label: "Discovered",
        color: None,
        get_from_test_state: |test_state| test_state.discovered,
        get_from_test_statues: |_test_statuses| &None,
    };
    pub const PASS: TestCounterColumn = TestCounterColumn {
        label: "Pass",
        color: Some(Color::Green),
        get_from_test_state: |test_state| test_state.pass,
        get_from_test_statues: |test_statuses| &test_statuses.passed,
    };
    pub const FAIL: TestCounterColumn = TestCounterColumn {
        label: "Fail",
        color: Some(Color::Red),
        get_from_test_state: |test_state| test_state.fail,
        get_from_test_statues: |test_statuses| &test_statuses.failed,
    };
    pub const FATAL: TestCounterColumn = TestCounterColumn {
        label: "Fatal",
        color: Some(Color::Red),
        get_from_test_state: |test_state| test_state.fatal,
        get_from_test_statues: |test_statuses| &test_statuses.fatals,
    };
    pub const SKIP: TestCounterColumn = TestCounterColumn {
        label: "Skip",
        color: Some(Color::Yellow),
        get_from_test_state: |test_state| test_state.skipped,
        get_from_test_statues: |test_statuses| &test_statuses.skipped,
    };
    pub const OMIT: TestCounterColumn = TestCounterColumn {
        label: "Omit",
        color: Some(Color::Magenta),
        get_from_test_state: |test_state| test_state.omitted,
        get_from_test_statues: |test_statuses| &test_statuses.omitted,
    };
    pub const TIMEOUT: TestCounterColumn = TestCounterColumn {
        label: "Timeout",
        color: Some(Color::Yellow),
        get_from_test_state: |test_state| test_state.timeout,
        get_from_test_statues: |test_statuses| &test_statuses.timed_out,
    };

    pub const INFRA_FAILURE: TestCounterColumn = TestCounterColumn {
        label: "Infra Failure",
        color: Some(Color::Magenta),
        get_from_test_state: |test_state| test_state.infra_failure,
        get_from_test_statues: |test_statuses| &test_statuses.infra_failure,
    };

    fn to_span_from_test_state(&self, test_state: &TestState) -> Result<Span, SpanError> {
        StylizedCount {
            label: self.label,
            count: (self.get_from_test_state)(test_state),
            color: self.color,
        }
        .to_span()
    }

    pub fn to_span_from_test_statuses(
        &self,
        test_statuses: &buck2_cli_proto::test_response::TestStatuses,
    ) -> buck2_error::Result<Span> {
        StylizedCount {
            label: self.label,
            count: (self.get_from_test_statues)(test_statuses)
                .as_ref()
                .with_internal_error(|| format!("Missing {} in TestStatuses", self.label))?
                .count,
            color: self.color,
        }
        .to_span()
        .map_err(|e| e.into())
    }
}

impl TestCounterComponent {
    fn draw_unchecked(
        &self,
        test_state: &TestState,
        _dimensions: Dimensions,
        mode: DrawMode,
    ) -> buck2_error::Result<Lines> {
        if matches!(mode, DrawMode::Final) {
            return Ok(Lines::new());
        }

        // TODO(brasselsprouts): use the outer try_into conversion on Lines.

        let mut spans = Vec::new();
        if test_state.listing_failed > 0 {
            spans.push(TestCounterColumn::LISTING_FAIL.to_span_from_test_state(test_state)?);
            spans.push(". ".try_into()?);
        }
        spans.push(TestCounterColumn::DISCOVERED.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::PASS.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::FAIL.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::FATAL.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::SKIP.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::OMIT.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::TIMEOUT.to_span_from_test_state(test_state)?);
        spans.push(". ".try_into()?);
        spans.push(TestCounterColumn::INFRA_FAILURE.to_span_from_test_state(test_state)?);
        Ok(Lines::from_iter([Line::from_iter(spans)]))
    }
}

/// Draw the test summary line above the `timed_list`
pub(crate) struct TestHeader<'a> {
    pub(crate) session_info: &'a SessionInfo,
    pub(crate) test_state: &'a TestState,
}

impl Component for TestHeader<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        dimensions: superconsole::Dimensions,
        mode: superconsole::DrawMode,
    ) -> buck2_error::Result<superconsole::Lines> {
        if self.session_info.test_session.is_some() {
            TestCounterComponent.draw_unchecked(self.test_state, dimensions, mode)
        } else {
            Ok(Lines::new())
        }
    }
}

pub fn span_from_build_failure_count(count: u64) -> Result<Span, SpanError> {
    StylizedCount {
        label: "Build failure",
        count,
        color: Some(Color::Red),
    }
    .to_span()
}

/// A count that receives color if and only if it's > 0
struct StylizedCount {
    label: &'static str,
    count: u64,
    color: Option<Color>,
}

impl StylizedCount {
    /// Turn this StylizedCount into a Superconsole Span.
    fn to_span(&self) -> Result<superconsole::Span, SpanError> {
        let mut style = ContentStyle::default();
        if self.count > 0 {
            style.foreground_color = self.color;
        }
        style
            .apply(format!("{} {}", self.label, self.count))
            .try_into()
    }
}
