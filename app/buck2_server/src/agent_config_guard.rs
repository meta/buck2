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
//! that configuration, so it is allowed and tagged with `CONFIG_CHANGE_EXPLICIT_TAG`. Every direct
//! agent call is also tagged with whether the guard was on, for A/B analysis.
//!
//! Only the daemon knows which configuration its state was built with, so this guard runs in
//! `DiceCommandUpdater::update`. That runs before the transaction commits, so blocking leaves the
//! state untouched.

use buck2_agent_interactive::Advice;
use buck2_agent_interactive::CONFIG_CHANGE_DISABLED_TAG;
use buck2_agent_interactive::CONFIG_CHANGE_ENABLED_TAG;
use buck2_agent_interactive::CONFIG_CHANGE_EXPLICIT_TAG;
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
    let arm_tag = if enabled {
        CONFIG_CHANGE_ENABLED_TAG
    } else {
        CONFIG_CHANGE_DISABLED_TAG
    };
    events.instant_event(buck2_data::TagEvent {
        tags: vec![arm_tag.to_owned()],
    });
    if !enabled {
        return Ok(());
    }
    let Some(diff) = existing
        .injected_external_buckconfig_data_change(new)
        .await?
    else {
        return Ok(());
    };
    if !diff.added_args.is_empty() {
        // The command chose new or different values. Record it so the volume of explicit switches
        // can be measured before deciding whether to guard them too.
        events.instant_event(buck2_data::TagEvent {
            tags: vec![CONFIG_CHANGE_EXPLICIT_TAG.to_owned()],
        });
        return Ok(());
    }
    // With nothing dropped, only files outside the repo changed or the same args were reordered;
    // neither is something the command left out.
    if diff.removed_args.is_empty() {
        return Ok(());
    }
    // The dropped args may only restate values the repo config already sets.
    if !existing.injected_root_buckconfig_values_change(new).await? {
        return Ok(());
    }

    let advice = Advice::ConfigChange;
    let decision = advice.evaluate(
        &Invocation {
            enabled,
            direct_call,
            reason: reason(agent_context),
        },
        is_advice_acknowledged(agent_context, advice.id()),
    );
    if let Some(tag) = decision.tag() {
        events.instant_event(buck2_data::TagEvent {
            tags: vec![tag.to_owned()],
        });
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
