/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Agent-interactive policy shared by client and daemon integration points.
//! Callers supply invocation facts and apply decisions through their own event/error paths.

/// Effective options for one command, independent of CLI parsing and daemon state.
pub struct Invocation<'a> {
    pub enabled: bool,
    pub direct_call: bool,
    pub reason: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildLanguage {
    Rust,
    Cxx,
}

impl BuildLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Cxx => "cxx",
        }
    }

    fn from_rule_type(rule_type: &str) -> Option<Self> {
        if rule_type.starts_with("rust_") {
            Some(Self::Rust)
        } else if rule_type.starts_with("cxx_") {
            Some(Self::Cxx)
        } else {
            None
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Rust => {
                "The direct build of this Rust target was blocked.\n\n\
                 If your goal is to verify Rust type correctness, use \
                 `arc rust-check <target>`.\n\n\
                 If your goal is to run tests, use `buck2 test` directly on the relevant \
                 test targets. It builds the tests and their dependencies before running \
                 them, so a separate build step is unnecessary.\n\n\
                 If this build is needed, retry with \
                 `--agent-context advice_ack=build` and \
                 `--agent-context reason=\"<why this Rust build is needed>\"`. \
                 This guard does not require user confirmation."
            }
            Self::Cxx => {
                "The direct build of this C++ target was blocked.\n\n\
                 If your goal is to verify C++ syntax and type correctness, use \
                 `arc cpp-check <target>`.\n\n\
                 If your goal is to run tests, use `buck2 test` directly on the relevant \
                 test targets. It builds the tests and their dependencies before running \
                 them, so a separate build step is unnecessary.\n\n\
                 If this build is needed, retry with \
                 `--agent-context advice_ack=build` and \
                 `--agent-context reason=\"<why this C++ build is needed>\"`. \
                 This guard does not require user confirmation."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Advice {
    Build(BuildLanguage),
    Killall,
    Clean,
    NewIsolationDir,
    /// The command drops config args that the daemon's state was built with, which changes its
    /// buckconfig without asking for different values. Evaluated in the daemon, which is the only
    /// place that knows the current configuration.
    ConfigChange,
}

/// Tag for a config change that the command asked for with its own config args. The daemon allows
/// such changes; the tag measures how often agents make them.
pub const CONFIG_CHANGE_EXPLICIT_TAG: &str = "agent_interactive.config_change.explicit";

/// Tags recording whether the config change guard was on for a direct agent call. Every direct
/// call that reaches the guard gets exactly one, so an A/B analysis can assign every command to an
/// arm, including the commands the guard never acts on.
pub const CONFIG_CHANGE_ENABLED_TAG: &str = "agent_interactive.config_change.enabled";
pub const CONFIG_CHANGE_DISABLED_TAG: &str = "agent_interactive.config_change.disabled";

/// Tag for a config change the command did not ask for: it drops config args the daemon's
/// configuration uses, which changes a resolved value. This is what the config change guard acts
/// on, and it is recorded in both arms of the guard's A/B test; `CONFIG_CHANGE_ENABLED_TAG` or
/// `CONFIG_CHANGE_DISABLED_TAG` on the same command says which arm.
pub const CONFIG_CHANGE_IMPLICIT_TAG: &str = "agent_interactive.config_change.implicit";

/// Tags recording whether the `agent_interactive` guards (killall, clean, new isolation directory)
/// were on for a direct agent call that reached the daemon. Those guards run in the client and tag
/// only the operations they guard, so these let an A/B analysis assign every other command to an
/// arm as well.
pub const AGENT_INTERACTIVE_ENABLED_TAG: &str = "agent_interactive.enabled";
pub const AGENT_INTERACTIVE_DISABLED_TAG: &str = "agent_interactive.disabled";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Inactive,
    Acknowledged(Advice),
    Block(Advice),
}

impl Advice {
    pub fn for_build_rule(rule_type: &str) -> Option<Self> {
        BuildLanguage::from_rule_type(rule_type).map(Self::Build)
    }

    /// Stable identifier used for acknowledgements and telemetry.
    pub fn id(self) -> &'static str {
        match self {
            Self::Build(_) => "build",
            Self::Killall => "killall",
            Self::Clean => "clean",
            Self::NewIsolationDir => "new_isolation_dir",
            Self::ConfigChange => "config_change",
        }
    }

    /// Tag recording that this guard's operation is about to happen in a direct agent call, and
    /// whether the guard is on. Callers record it after the same eligibility checks in both arms of
    /// the guard's A/B test, so the operations of the control arm can be compared with those the
    /// guard acted on. The build guard records the same names.
    pub fn state_tag(self, enabled: bool) -> String {
        let state = if enabled { "enabled" } else { "disabled" };
        format!("agent_interactive.{}.{state}", self.id())
    }

    /// `acknowledged` must refer to this advice ID in the current invocation.
    pub fn evaluate(self, invocation: &Invocation<'_>, acknowledged: bool) -> Decision {
        if !invocation.enabled || !invocation.direct_call {
            return Decision::Inactive;
        }
        if acknowledged
            && invocation
                .reason
                .is_some_and(|reason| !reason.trim().is_empty())
        {
            Decision::Acknowledged(self)
        } else {
            Decision::Block(self)
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Build(language) => language.message(),
            Self::Clean => {
                "No build state or outputs have been removed.\n\n\
                 `buck2 clean` stops the daemon and removes reusable build state and outputs, \
                 making subsequent builds more expensive. Buck tracks input changes automatically; \
                 retry the build or test without cleaning first. Use `buck2 clean --dry-run` \
                 to inspect the affected paths, or `buck2 clean --stale` to reclaim stale outputs.\n\n\
                 If a full clean is necessary, retry with `--agent-context advice_ack=clean` \
                 and `--agent-context reason=\"<why build state must be cleared>\"`. \
                 This guard does not require user confirmation."
            }
            Self::NewIsolationDir => {
                "No daemon has been started for this isolation directory.\n\n\
                 A new isolation directory starts a separate daemon, consumes additional memory, \
                 and loses reuse of existing build state. Prefer the default isolation directory \
                 or reuse the isolation directory already used for this work. Check both \
                 `--isolation-dir` and `BUCK_ISOLATION_DIR` when choosing the directory.\n\n\
                 If a new isolation directory is necessary, retry with \
                 `--agent-context advice_ack=new_isolation_dir` and \
                 `--agent-context reason=\"<why separate daemon state is needed>\"`. \
                 This guard does not require user confirmation."
            }
            Self::Killall => {
                "No processes have been killed.\n\n\
                 `buck2 killall` discards reusable daemon state and can interrupt other work. \
                 Inspect `buck2 status` first. If one daemon needs restarting, prefer `buck2 kill` \
                 in its repository with the appropriate `--isolation-dir`.\n\
                 To limit `killall`, use `--repo` and/or `--in-isolation-dir`; \
                 the global `--isolation-dir` flag does not limit which processes it kills.\n\n\
                 If killall is necessary, retry with `--agent-context advice_ack=killall` \
                 and `--agent-context reason=\"<why killall is needed>\"`. \
                 This guard does not require user confirmation."
            }
            Self::ConfigChange => {
                "This command has not run, and the daemon's build state is unchanged.\n\n\
                 It drops config arguments that the daemon's current configuration uses (listed \
                 below), which changes that configuration. Running it invalidates cached parsing \
                 and analysis results, and the next command that passes those arguments again pays \
                 that cost again. If the change is not needed, add `--reuse-current-config` to keep \
                 the current configuration (this ignores the command's config arguments).\n\n\
                 If the configuration change is necessary, retry with \
                 `--agent-context advice_ack=config_change` and \
                 `--agent-context reason=\"<why this configuration is needed>\"`. \
                 This guard does not require user confirmation."
            }
        }
    }
}

impl Decision {
    pub fn tag(&self) -> Option<&'static str> {
        match self {
            Self::Inactive => None,
            Self::Acknowledged(Advice::Build(_)) => Some("agent_interactive.build.ack"),
            Self::Block(Advice::Build(_)) => Some("agent_interactive.build.block"),
            Self::Acknowledged(Advice::Killall) => Some("agent_interactive.killall.ack"),
            Self::Block(Advice::Killall) => Some("agent_interactive.killall.block"),
            Self::Acknowledged(Advice::Clean) => Some("agent_interactive.clean.ack"),
            Self::Block(Advice::Clean) => Some("agent_interactive.clean.block"),
            Self::Acknowledged(Advice::NewIsolationDir) => {
                Some("agent_interactive.new_isolation_dir.ack")
            }
            Self::Block(Advice::NewIsolationDir) => {
                Some("agent_interactive.new_isolation_dir.block")
            }
            Self::Acknowledged(Advice::ConfigChange) => Some("agent_interactive.config_change.ack"),
            Self::Block(Advice::ConfigChange) => Some("agent_interactive.config_change.block"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_ADVICE: [Advice; 6] = [
        Advice::Build(BuildLanguage::Rust),
        Advice::Build(BuildLanguage::Cxx),
        Advice::Killall,
        Advice::Clean,
        Advice::NewIsolationDir,
        Advice::ConfigChange,
    ];

    #[test]
    fn build_advice_depends_on_rule_type() {
        for (rule_type, expected) in [
            ("rust_library", Some(Advice::Build(BuildLanguage::Rust))),
            ("rust_binary", Some(Advice::Build(BuildLanguage::Rust))),
            ("cxx_library", Some(Advice::Build(BuildLanguage::Cxx))),
            ("cxx_binary", Some(Advice::Build(BuildLanguage::Cxx))),
            ("rust_test", Some(Advice::Build(BuildLanguage::Rust))),
            ("cxx_test", Some(Advice::Build(BuildLanguage::Cxx))),
            ("rust_custom", Some(Advice::Build(BuildLanguage::Rust))),
            ("cxx_custom", Some(Advice::Build(BuildLanguage::Cxx))),
            ("rust", None),
            ("cxx", None),
            ("custom_rust_binary", None),
            ("custom_cxx_binary", None),
            ("genrule", None),
            ("custom_test", None),
        ] {
            assert_eq!(Advice::for_build_rule(rule_type), expected, "{rule_type}");
        }
    }

    #[test]
    fn only_enabled_direct_calls_are_guarded() {
        for advice in ALL_ADVICE {
            for (enabled, direct_call) in [(false, false), (false, true), (true, false)] {
                let invocation = Invocation {
                    enabled,
                    direct_call,
                    reason: None,
                };
                assert_eq!(advice.evaluate(&invocation, false), Decision::Inactive);
            }
        }
    }

    #[test]
    fn state_tags_name_the_guard_and_its_state() {
        assert_eq!(
            Advice::Killall.state_tag(true),
            "agent_interactive.killall.enabled"
        );
        assert_eq!(
            Advice::NewIsolationDir.state_tag(false),
            "agent_interactive.new_isolation_dir.disabled"
        );
        // Matches the names the build guard records for every language.
        assert_eq!(
            Advice::Build(BuildLanguage::Rust).state_tag(false),
            "agent_interactive.build.disabled"
        );
    }

    #[test]
    fn proceeding_requires_acknowledgement_and_reason() {
        for advice in ALL_ADVICE {
            for (reason, expected) in [
                (None, Decision::Block(advice)),
                (Some(""), Decision::Block(advice)),
                (Some(" \n\t"), Decision::Block(advice)),
                (
                    Some("Several daemons are unresponsive"),
                    Decision::Acknowledged(advice),
                ),
            ] {
                let invocation = Invocation {
                    enabled: true,
                    direct_call: true,
                    reason,
                };
                assert_eq!(advice.evaluate(&invocation, false), Decision::Block(advice));
                assert_eq!(advice.evaluate(&invocation, true), expected);
            }
        }
    }
}
