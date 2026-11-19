/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_settings_derive::buck_settings_section;

/// Block direct agent calls whose buckconfig would invalidate the daemon's state.
///
/// This is its own section rather than a key in `agent_interactive` so it can roll out without
/// bumping that section's version: rollouts are selected by exact version, so a bump would stop
/// the existing rollouts of the other guards from applying. The guard also ignores the
/// `agent_interactive.enabled` master switch, so this section alone controls it and its GK can
/// be A/B tested on its own.
#[buck_settings_section(name = "agent_config_change", version = 0, scope = invocation)]
pub struct AgentConfigChange {
    /// The guard's only switch.
    #[setting(default = false, overridable_in(CommandLine, LocalSettings))]
    enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use crate::BuckSettings;
    use crate::parser::resolve_setting_flags;
    use crate::parser::table;

    #[test]
    fn disabled_unless_set() -> buck2_error::Result<()> {
        assert!(
            !BuckSettings::empty()
                .invocation
                .agent_config_change
                .enabled()
        );
        for (value, expected) in [(true, true), (false, false)] {
            let settings = resolve_setting_flags(vec![table(&format!(
                "[agent_config_change]\nenabled = {value}",
            ))])?;
            assert_eq!(settings.invocation.agent_config_change.enabled(), expected);
            // Setting this section does not turn on the master switch, which the guard ignores.
            assert!(!settings.invocation.agent_interactive.enabled());
        }
        Ok(())
    }
}
