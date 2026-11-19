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

/// Guards on operations an agent runs interactively. Each guard needs both
/// the master switch and its own.
#[buck_settings_section(name = "agent_interactive", version = 1, scope = invocation)]
pub struct AgentInteractive {
    #[setting(default = false, overridable_in(CommandLine, LocalSettings))]
    enabled: Option<bool>,
    #[setting(
        default = false,
        overridable_in(CommandLine, LocalSettings),
        accessor = killall_switch
    )]
    killall_enabled: Option<bool>,
    #[setting(
        default = false,
        overridable_in(CommandLine, LocalSettings),
        accessor = clean_switch
    )]
    clean_enabled: Option<bool>,
    #[setting(
        default = false,
        overridable_in(CommandLine, LocalSettings),
        accessor = new_isolation_dir_switch
    )]
    new_isolation_dir_enabled: Option<bool>,
}

impl AgentInteractive {
    pub fn killall_enabled(&self) -> bool {
        self.enabled() && self.killall_switch()
    }

    pub fn clean_enabled(&self) -> bool {
        self.enabled() && self.clean_switch()
    }

    pub fn new_isolation_dir_enabled(&self) -> bool {
        self.enabled() && self.new_isolation_dir_switch()
    }
}

#[cfg(test)]
mod tests {
    use crate::BuckSettings;
    use crate::parser::resolve_setting_flags;
    use crate::parser::table;

    #[test]
    fn guards_require_both_switches() -> buck2_error::Result<()> {
        let defaults = BuckSettings::empty().invocation.agent_interactive;
        assert!(!defaults.enabled());
        assert!(!defaults.killall_enabled());
        assert!(!defaults.clean_enabled());
        assert!(!defaults.new_isolation_dir_enabled());

        for master in [false, true] {
            for name in [
                "killall_enabled",
                "clean_enabled",
                "new_isolation_dir_enabled",
            ] {
                let settings = resolve_setting_flags(vec![table(&format!(
                    "[agent_interactive]\nenabled = {master}\n{name} = true",
                ))])?
                .invocation
                .agent_interactive;
                assert_eq!(settings.enabled(), master);
                assert_eq!(
                    settings.killall_enabled(),
                    master && name == "killall_enabled"
                );
                assert_eq!(settings.clean_enabled(), master && name == "clean_enabled");
                assert_eq!(
                    settings.new_isolation_dir_enabled(),
                    master && name == "new_isolation_dir_enabled"
                );
            }
        }
        let master_only =
            resolve_setting_flags(vec![table("[agent_interactive]\nenabled = true")])?
                .invocation
                .agent_interactive;
        assert!(!master_only.killall_enabled());
        assert!(!master_only.clean_enabled());
        assert!(!master_only.new_isolation_dir_enabled());
        Ok(())
    }
}
