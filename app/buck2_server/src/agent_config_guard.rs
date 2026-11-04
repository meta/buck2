/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

//! Agent-interactive guard for commands that implicitly change the daemon's buckconfig.
//!
//! Every command injects its own external buckconfig data into DICE, so a command whose
//! `--config`/`--config-file`/mode arguments differ from the previous command's invalidates
//! buckconfig-dependent state, and returning to the previous configuration pays that cost again.
//! The guard blocks only the change an agent did not ask for: a command that drops args the
//! daemon's configuration uses, such as `uquery` without the preceding build's `-c` flags, and
//! thereby changes a resolved value. A command that passes its own new or different args chose
//! that configuration, so it is allowed and tagged with `CONFIG_CHANGE_EXPLICIT_TAG`.
//!
//! The guard is rolled out as an A/B test. Every direct agent call is tagged with whether the guard
//! was on, and the implicit and explicit change tags are recorded in both arms, so the arms can be
//! compared event by event. With the guard off, that classification is telemetry only and never
//! fails a command.
//!
//! Only the daemon knows which configuration its state was built with, so this guard runs in
//! `DiceCommandUpdater::update`. That runs before the transaction commits, so blocking leaves the
//! state untouched.

use buck2_agent_interactive::AGENT_INTERACTIVE_DISABLED_TAG;
use buck2_agent_interactive::AGENT_INTERACTIVE_ENABLED_TAG;
use buck2_agent_interactive::Advice;
use buck2_agent_interactive::CONFIG_CHANGE_DISABLED_TAG;
use buck2_agent_interactive::CONFIG_CHANGE_ENABLED_TAG;
use buck2_agent_interactive::CONFIG_CHANGE_EXPLICIT_TAG;
use buck2_agent_interactive::CONFIG_CHANGE_IMPLICIT_TAG;
use buck2_agent_interactive::Decision;
use buck2_agent_interactive::Invocation;
use buck2_common::legacy_configs::cells::ExternalBuckconfigData;
use buck2_common::legacy_configs::cells::ExternalBuckconfigDataDiff;
use buck2_common::legacy_configs::dice::HasInjectedLegacyConfigs;
use buck2_data::AgentContextEntry;
use buck2_error::ErrorTag;
use buck2_events::dispatch::EventDispatcher;
use buck2_settings::InvocationBuckSettings;
use dice::DiceComputations;

/// Blocks a direct agent call that drops config args injected in `existing` and thereby changes a
/// resolved root cell buckconfig value, unless the call acknowledges `config_change` with a reason.
///
/// `--reuse-current-config` needs no special case: `load_new_configs` then returns the injected
/// data itself, which compares equal.
pub(crate) async fn check_agent_config_change(
    existing: &mut DiceComputations<'_>,
    new: &ExternalBuckconfigData,
    settings: &InvocationBuckSettings,
    agent_context: &[AgentContextEntry],
    events: &EventDispatcher,
) -> buck2_error::Result<()> {
    // Check the cheap conditions first: comparing configs clones the new data, and every
    // command that is not a guarded agent call should skip that.
    let direct_call = is_direct_call(agent_context);
    if !direct_call {
        return Ok(());
    }
    // Unlike the other agent guards, this one ignores the `agent_interactive.enabled` master
    // switch. Requiring both would limit it to the intersection of two GK rollouts and leave no
    // clean control group for an A/B test of this guard.
    let enabled = settings.agent_config_change.enabled();
    // Tag every direct call with the guard's state, not only the calls it acts on, so an A/B
    // analysis can compare all commands in each arm.
    tag(
        events,
        if enabled {
            CONFIG_CHANGE_ENABLED_TAG
        } else {
            CONFIG_CHANGE_DISABLED_TAG
        },
    );

    let change = match classify_change(existing, new).await {
        Ok(change) => change,
        Err(e) if enabled => return Err(e),
        // With the guard off the classification is telemetry only.
        Err(e) => {
            tracing::debug!("Skipping config change classification for a disabled guard: {e:#}");
            return Ok(());
        }
    };
    let diff = match change {
        ConfigChange::Unchanged => return Ok(()),
        ConfigChange::Explicit => {
            // Recorded so the volume of explicit switches can be measured, and compared between
            // arms, before deciding whether to guard them too.
            tag(events, CONFIG_CHANGE_EXPLICIT_TAG);
            return Ok(());
        }
        ConfigChange::Implicit(diff) => {
            tag(events, CONFIG_CHANGE_IMPLICIT_TAG);
            diff
        }
    };

    let advice = Advice::ConfigChange;
    // `Decision::Inactive` when the guard is off.
    let decision = advice.evaluate(
        &Invocation {
            enabled,
            direct_call,
            reason: reason(agent_context),
        },
        is_advice_acknowledged(agent_context, advice.id()),
    );
    if let Some(decision_tag) = decision.tag() {
        tag(events, decision_tag);
    }
    match decision {
        Decision::Inactive | Decision::Acknowledged(_) => Ok(()),
        Decision::Block(advice) => Err(buck2_error::buck2_error!(
            ErrorTag::AgentAdviceBlocked,
            "Buck advice [{}]\n{}\n\n{}",
            advice.id(),
            advice.message(),
            describe_dropped_args(&diff),
        )),
    }
}

/// Tags a direct agent call with whether the `agent_interactive` guards were on. Those guards run
/// in the client and tag only the operations they guard; this assigns the call to an arm of their
/// A/B test even when it is not one of those operations.
pub(crate) fn tag_agent_interactive_arm(
    settings: &InvocationBuckSettings,
    agent_context: &[AgentContextEntry],
    events: &EventDispatcher,
) {
    if !is_direct_call(agent_context) {
        return;
    }
    tag(
        events,
        if settings.agent_interactive.enabled() {
            AGENT_INTERACTIVE_ENABLED_TAG
        } else {
            AGENT_INTERACTIVE_DISABLED_TAG
        },
    );
}

/// How the config a command is about to inject differs from the injected config, as far as the
/// guard is concerned.
enum ConfigChange {
    /// No change, or none the command left out: only files outside the repo changed, the same args
    /// were reordered, or the dropped args only restated values the repo config already sets.
    Unchanged,
    /// The command passed new or different args, so it chose the new configuration.
    Explicit,
    /// The command dropped args and that changes a resolved value.
    Implicit(ExternalBuckconfigDataDiff),
}

async fn classify_change(
    existing: &mut DiceComputations<'_>,
    new: &ExternalBuckconfigData,
) -> buck2_error::Result<ConfigChange> {
    let Some(diff) = existing
        .injected_external_buckconfig_data_change(new)
        .await?
    else {
        return Ok(ConfigChange::Unchanged);
    };
    if !diff.added_args.is_empty() {
        return Ok(ConfigChange::Explicit);
    }
    if diff.removed_args.is_empty() {
        return Ok(ConfigChange::Unchanged);
    }
    if !existing.injected_root_buckconfig_values_change(new).await? {
        return Ok(ConfigChange::Unchanged);
    }
    Ok(ConfigChange::Implicit(diff))
}

fn tag(events: &EventDispatcher, tag: &str) {
    events.instant_event(buck2_data::TagEvent {
        tags: vec![tag.to_owned()],
    });
}

/// Lists the args the command drops, which `--reuse-current-config` would keep.
fn describe_dropped_args(diff: &ExternalBuckconfigDataDiff) -> String {
    let mut lines =
        vec!["Config arguments the daemon is using that this command drops:".to_owned()];
    lines.extend(diff.removed_args.iter().map(|arg| format!("  {arg}")));
    if !diff.changed_external_files.is_empty() {
        lines.push("Config files outside the repo that also changed:".to_owned());
        lines.extend(
            diff.changed_external_files
                .iter()
                .map(|path| format!("  {path}")),
        );
    }
    lines.join("\n")
}

// These read entries the same way as `buck2_client_ctx::agent_context::AgentContext`, which the
// client-side guards use; keep the two in sync.

/// The last direct-call marker determines whether the call is direct.
fn is_direct_call(entries: &[AgentContextEntry]) -> bool {
    entries
        .iter()
        .rev()
        .find(|entry| entry.key == AgentContextEntry::KEY_DIRECT_CALL)
        .is_some_and(|entry| entry.value == "true")
}

/// An invocation can acknowledge multiple advice IDs through repeated entries.
fn is_advice_acknowledged(entries: &[AgentContextEntry], advice_id: &str) -> bool {
    entries
        .iter()
        .any(|entry| entry.key == AgentContextEntry::KEY_ADVICE_ACK && entry.value == advice_id)
}

/// Later CLI entries can replace a reason supplied by a wrapper.
fn reason(entries: &[AgentContextEntry]) -> Option<&str> {
    entries
        .iter()
        .rev()
        .find(|entry| entry.key == "reason")
        .map(|entry| entry.value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(pairs: &[(&str, &str)]) -> Vec<AgentContextEntry> {
        pairs
            .iter()
            .map(|(key, value)| AgentContextEntry {
                key: (*key).to_owned(),
                value: (*value).to_owned(),
            })
            .collect()
    }

    #[test]
    fn reads_agent_context_like_the_client() {
        let context = entries(&[
            ("direct_call", "false"),
            ("reason", "from a wrapper"),
            ("advice_ack", "killall"),
            ("direct_call", "true"),
            ("advice_ack", "config_change"),
            ("reason", "needs the release mode"),
        ]);
        assert!(is_direct_call(&context));
        assert!(is_advice_acknowledged(&context, "config_change"));
        assert!(is_advice_acknowledged(&context, "killall"));
        assert!(!is_advice_acknowledged(&context, "clean"));
        assert_eq!(reason(&context), Some("needs the release mode"));

        let empty = entries(&[]);
        assert!(!is_direct_call(&empty));
        assert_eq!(reason(&empty), None);
        assert!(!is_direct_call(&entries(&[("direct_call", "1")])));
    }

    #[test]
    fn describes_dropped_args_and_changed_files() {
        let mut diff = ExternalBuckconfigDataDiff {
            added_args: Vec::new(),
            removed_args: vec![
                "--config foo.bar=1".to_owned(),
                "--config-file mode/dev".to_owned(),
            ],
            changed_external_files: Vec::new(),
        };
        assert_eq!(
            describe_dropped_args(&diff),
            "Config arguments the daemon is using that this command drops:\n  \
             --config foo.bar=1\n  \
             --config-file mode/dev"
        );
        diff.changed_external_files = vec!["/etc/buckconfig.d/site".to_owned()];
        assert!(describe_dropped_args(&diff).ends_with(
            "Config files outside the repo that also changed:\n  /etc/buckconfig.d/site"
        ));
    }
}
