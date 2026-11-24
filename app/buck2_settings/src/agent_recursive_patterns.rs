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

/// Requires explicit intent for recursive build and test patterns in the
/// commands an AI agent runs directly (those given `--agent-context
/// direct_call=true`). This guard does not need `agent_interactive.enabled`.
// Independent of the interactive master switch so the rollout has its own control group.
#[buck_settings_section(name = "agent_recursive_patterns", version = 0, scope = invocation)]
pub struct AgentRecursivePatterns {
    /// Stops an AI agent's direct `buck2 build` or `buck2 test` of a
    /// recursive pattern such as `//dir/...` until the agent acknowledges
    /// that it needs the whole scope.
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
    fn independent_of_interactive_master() -> buck2_error::Result<()> {
        assert!(
            !BuckSettings::empty()
                .invocation
                .agent_recursive_patterns
                .enabled()
        );
        for enabled in [false, true] {
            for master in [false, true] {
                let settings = resolve_setting_flags(vec![table(&format!(
                    "[agent_recursive_patterns]\nenabled = {enabled}\n[agent_interactive]\nenabled = {master}",
                ))])?;
                assert_eq!(
                    settings.invocation.agent_recursive_patterns.enabled(),
                    enabled
                );
                assert_eq!(settings.invocation.agent_interactive.enabled(), master);
            }
        }
        Ok(())
    }
}
