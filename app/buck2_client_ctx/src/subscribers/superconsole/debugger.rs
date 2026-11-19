/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_event_observer::starlark_debug::StarlarkDebuggerState;
use superconsole::Component;
use superconsole::Lines;

#[derive(Debug)]
pub(crate) struct StarlarkDebuggerComponent<'s> {
    pub(crate) starlark_debugger_state: &'s StarlarkDebuggerState,
}

impl Component for StarlarkDebuggerComponent<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        _dimensions: superconsole::Dimensions,
        _mode: superconsole::DrawMode,
    ) -> buck2_error::Result<superconsole::Lines> {
        let state = self.starlark_debugger_state;

        if !state.debugger_attached {
            return Ok(Lines::new());
        }

        let mut lines: Vec<String> = Vec::new();
        lines.push("     ******** Buck2 Starlark Debugger is attached **********".to_owned());

        let total = state.this_stopped_evals.len() + state.other_stopped_evals.len();
        for stopped in &state.this_stopped_evals {
            if lines.len() < 7 {
                lines.push(format!(
                    "( stopped)   {:<60} @ {}",
                    stopped.description, stopped.stopped_at
                ));
            }
        }
        for stopped in &state.other_stopped_evals {
            if lines.len() < 7 {
                lines.push(format!(
                    "(*stopped) {:<60} @ {}",
                    stopped.description, stopped.stopped_at
                ));
            }
        }

        let shown = lines.len() - 1;
        if total > shown {
            lines.push(format!("{} more paused evaluations", total - shown));
        }

        lines.push("      *******************************************************".to_owned());

        Ok(Lines(
            lines
                .into_iter()
                .map(|v| vec![v].try_into())
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }
}
