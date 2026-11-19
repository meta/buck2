/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_event_observer::dice_state::DiceState;
use buck2_event_observer::humanized::HumanizedCount;
use superconsole::Component;
use superconsole::Lines;

use crate::subscribers::superconsole::SuperConsoleConfig;

pub(crate) struct DiceComponent<'s> {
    pub(crate) super_console_config: &'s SuperConsoleConfig,
    pub(crate) dice_state: &'s DiceState,
}

impl Component for DiceComponent<'_> {
    type Error = buck2_error::Error;

    fn draw_unchecked(
        &self,
        _dimensions: superconsole::Dimensions,
        _mode: superconsole::DrawMode,
    ) -> buck2_error::Result<superconsole::Lines> {
        if !self.super_console_config.enable_dice {
            return Ok(Lines::new());
        }

        let mut lines = vec!["Dice Key States".to_owned()];

        let header = format!(
            "  {:<42}  {:>6}  {:>6}  {:>6}  {:>6}",
            "  Key", "ChkDeps", "Compute", "Pending", "Done"
        );
        let header_len = header.len();
        lines.push(header);
        lines.push("-".repeat(header_len));
        for (k, v) in self.dice_state.key_states() {
            // We aren't guaranteed to get a final update, the final rendering might look a little
            // silly with some keys claiming to be in progress, but this is a debug component so
            // that is probably OK.
            let check_deps = v.check_deps_started - v.check_deps_finished;
            let computing = v.compute_started - v.compute_finished;
            let pending = v.started - v.finished;
            let finished = v.finished;

            lines.push(format!(
                "    {:<40} |  {}  |  {}  |  {}  |  {}",
                // Dice key states are all ascii
                if k.len() > 40 { &k[..40] } else { k },
                HumanizedCount::fixed_width(check_deps.into()),
                HumanizedCount::fixed_width(computing.into()),
                HumanizedCount::fixed_width(pending.into()),
                HumanizedCount::fixed_width(finished.into())
            ));
        }
        lines.push("-".repeat(header_len));
        Ok(Lines(
            lines
                .into_iter()
                .map(|v| vec![v].try_into())
                .collect::<Result<Vec<_>, _>>()?,
        ))
    }
}
