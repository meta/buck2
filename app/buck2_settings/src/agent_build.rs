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

/// Rule-specific check and test advice for the builds an AI agent runs
/// directly (commands given `--agent-context direct_call=true`) that request
/// no artifacts.
// A separate section preserves the exact-version rollouts of `agent_interactive`.
#[buck_settings_section(name = "agent_build", version = 0, scope = invocation)]
pub struct AgentBuild {
    /// Stops an AI agent's direct `buck2 build` that asks for no outputs when
    /// the rule of a target has a check or test to recommend instead, until
    /// the agent acknowledges the advice.
    #[setting(
        internal_default = false,
        oss_default = false,
        overridable_in(CommandLine, LocalSettings)
    )]
    enabled: Option<bool>,
}

#[cfg(test)]
mod tests {
    use crate::BuckSettings;
    use crate::parser::resolve_setting_flags;
    use crate::parser::table;

    #[test]
    fn disabled_unless_set() -> buck2_error::Result<()> {
        assert!(!BuckSettings::empty().invocation.agent_build.enabled());
        for enabled in [false, true] {
            let settings = resolve_setting_flags(vec![table(&format!(
                "[agent_build]\nenabled = {enabled}",
            ))])?;
            assert_eq!(settings.invocation.agent_build.enabled(), enabled);
            assert!(!settings.invocation.agent_interactive.enabled());
        }
        Ok(())
    }
}
