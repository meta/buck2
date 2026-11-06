/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;
use buck2_e2e::utils::json_get;
use serde_json::Value;

const ENABLED: [&str; 1] = ["--setting=agent_config_change.enabled=true"];
const DIRECT: &str = "--agent-context=direct_call=true";
const ADVICE: &str = r"Buck advice \[config_change\]";
const EXPLICIT_TAG: &str = "agent_interactive.config_change.explicit";
const ENABLED_TAG: &str = "agent_interactive.config_change.enabled";
const DISABLED_TAG: &str = "agent_interactive.config_change.disabled";
const BLOCK_TAG: &str = "agent_interactive.config_change.block";
const IMPLICIT_TAG: &str = "agent_interactive.config_change.implicit";
const INTERACTIVE_ENABLED_TAG: &str = "agent_interactive.enabled";
const INTERACTIVE_DISABLED_TAG: &str = "agent_interactive.disabled";

/// `//:` with `args`, then [`ENABLED`] and [`DIRECT`].
fn targets_args<'a>(args: &[&'a str]) -> Vec<&'a str> {
    [&["//:"], args, &ENABLED, &[DIRECT]].concat()
}

fn has_tag(record: &Value, tag: &str) -> bool {
    record["tags"].as_array().unwrap().contains(&tag.into())
}

#[buck_test(write_invocation_record = true)]
async fn test_dropping_config_args_is_blocked_and_keeps_daemon_state(
    buck: &Buck,
) -> anyhow::Result<()> {
    buck.targets(&targets_args(&["-c", "foo.bar=1"])).await?;

    // Like `uquery` without the preceding build's flags.
    let result = buck.targets(&targets_args(&[])).await.expect_failure();
    result.assert_stderr_matches(ADVICE);
    assert!(result.stderr.contains("that this command drops:"));
    assert!(result.stderr.contains("--config foo.bar=1"));
    let record = result.invocation_record()?;
    assert!(has_tag(&record, IMPLICIT_TAG));
    assert!(has_tag(&record, BLOCK_TAG));
    assert!(has_tag(&record, ENABLED_TAG));
    assert!(has_tag(record.single_error(), "AGENT_ADVICE_BLOCKED"));

    // The blocked command never committed, so returning to the original config finds the
    // daemon's state as the first command left it.
    let after = buck.targets(&targets_args(&["-c", "foo.bar=1"])).await?;
    let record = after.invocation_record()?;
    assert!(json_get(&record, &["previous_uuid_with_mismatched_config"]).is_none());
    assert_eq!(record["new_configs_used"], false);
    Ok(())
}

#[buck_test]
async fn test_dropping_some_config_args_is_blocked(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&targets_args(&["-c", "foo.bar=1", "-c", "foo.baz=2"]))
        .await?;
    let result = buck
        .targets(&targets_args(&["-c", "foo.bar=1"]))
        .await
        .expect_failure();
    result.assert_stderr_matches(ADVICE);
    assert!(result.stderr.contains("--config foo.baz=2"));
    assert!(!result.stderr.contains("--config foo.bar=1"));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_explicit_config_changes_are_allowed_and_tagged(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&targets_args(&["-c", "foo.bar=1"])).await?;
    for args in [
        // A different value for the same key.
        &["-c", "foo.bar=2"][..],
        // An additional key.
        &["-c", "foo.bar=2", "-c", "foo.baz=3"],
    ] {
        let result = buck.targets(&targets_args(args)).await?;
        assert!(!result.stderr.contains("Buck advice"));
        assert!(has_tag(&*result.invocation_record()?, EXPLICIT_TAG));
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_dropping_args_that_restate_repo_values_is_allowed(buck: &Buck) -> anyhow::Result<()> {
    // The fixture's `.buckconfig` already sets `test.default = 2`.
    buck.targets(&targets_args(&["-c", "test.default=2"]))
        .await?;
    let result = buck.targets(&targets_args(&[])).await?;
    assert!(!result.stderr.contains("Buck advice"));
    assert!(!has_tag(&*result.invocation_record()?, EXPLICIT_TAG));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_drop_requires_its_own_ack_and_nonempty_reason(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&targets_args(&["-c", "foo.bar=1"])).await?;
    for context in [
        "advice_ack=killall,reason=Needs the repo defaults",
        "advice_ack=config_change",
        "advice_ack=config_change,reason=   ",
        "reason=Needs the repo defaults",
    ] {
        let context_arg = format!("--agent-context={context}");
        buck.targets(&[&targets_args(&[])[..], &[&context_arg]].concat())
            .await
            .expect_failure()
            .assert_stderr_matches(ADVICE);
    }

    let result = buck
        .targets(
            &[
                &targets_args(&[])[..],
                &["--agent-context=advice_ack=config_change,reason=Needs the repo defaults"],
            ]
            .concat(),
        )
        .await?;
    let record = result.invocation_record()?;
    assert!(has_tag(&record, "agent_interactive.config_change.ack"));
    assert!(
        record["previous_uuid_with_mismatched_config"]
            .as_str()
            .is_some_and(|uuid| !uuid.is_empty())
    );
    Ok(())
}

#[buck_test]
async fn test_unchanged_config_is_not_blocked(buck: &Buck) -> anyhow::Result<()> {
    // The first command after daemon start has no state to invalidate.
    for args in [
        &["-c", "foo.bar=1"][..],
        &["-c", "foo.bar=1"],
        // Keeps the daemon's configuration even though the command drops `-c foo.bar=1`.
        &["--reuse-current-config"],
    ] {
        let result = buck.targets(&targets_args(args)).await?;
        assert!(!result.stderr.contains("Buck advice"));
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_direct_calls_record_whether_the_guard_is_on(buck: &Buck) -> anyhow::Result<()> {
    for (args, expected) in [
        ([&ENABLED[..], &[DIRECT]].concat(), ENABLED_TAG),
        (vec![DIRECT], DISABLED_TAG),
        (
            vec!["--setting=agent_config_change.enabled=false", DIRECT],
            DISABLED_TAG,
        ),
    ] {
        let record = buck
            .targets(&[&["//:"], &args[..]].concat())
            .await?
            .invocation_record()?;
        assert!(has_tag(&record, expected), "{args:?}");
        assert!(
            [ENABLED_TAG, DISABLED_TAG]
                .into_iter()
                .filter(|tag| *tag != expected)
                .all(|tag| !has_tag(&record, tag)),
            "{args:?}"
        );
    }

    // The agent_interactive guards' arm is recorded the same way.
    for (args, expected) in [
        (
            &["--setting=agent_interactive.enabled=true", DIRECT][..],
            INTERACTIVE_ENABLED_TAG,
        ),
        (&[DIRECT], INTERACTIVE_DISABLED_TAG),
    ] {
        let record = buck
            .targets(&[&["//:"], args].concat())
            .await?
            .invocation_record()?;
        assert!(has_tag(&record, expected), "{args:?}");
        assert!(
            [INTERACTIVE_ENABLED_TAG, INTERACTIVE_DISABLED_TAG]
                .into_iter()
                .filter(|tag| *tag != expected)
                .all(|tag| !has_tag(&record, tag)),
            "{args:?}"
        );
    }

    // Only direct agent calls are in the experiment population.
    let arm_tags = [
        ENABLED_TAG,
        DISABLED_TAG,
        INTERACTIVE_ENABLED_TAG,
        INTERACTIVE_DISABLED_TAG,
    ];
    for args in [
        ENABLED.to_vec(),
        [&ENABLED[..], &["--agent-context=direct_call=false"]].concat(),
    ] {
        let record = buck
            .targets(&[&["//:"], &args[..]].concat())
            .await?
            .invocation_record()?;
        assert!(
            arm_tags.into_iter().all(|tag| !has_tag(&record, tag)),
            "{args:?}"
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_disabled_guard_records_the_same_events(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&["//:", "-c", "foo.bar=1", DIRECT]).await?;

    // The implicit change the guard would block is allowed and recorded with the control arm.
    let result = buck.targets(&["//:", DIRECT]).await?;
    assert!(!result.stderr.contains("Buck advice"));
    let record = result.invocation_record()?;
    assert!(has_tag(&record, IMPLICIT_TAG) && has_tag(&record, DISABLED_TAG));
    assert!(!has_tag(&record, BLOCK_TAG));

    // An explicit change is recorded the same way in both arms.
    let record = buck
        .targets(&["//:", "-c", "test.default=2", DIRECT])
        .await?
        .invocation_record()?;
    assert!(has_tag(&record, EXPLICIT_TAG) && has_tag(&record, DISABLED_TAG));

    // Dropping an arg that restates the repo value changes nothing, so nothing is recorded.
    let record = buck.targets(&["//:", DIRECT]).await?.invocation_record()?;
    assert!(!has_tag(&record, IMPLICIT_TAG) && !has_tag(&record, EXPLICIT_TAG));

    // Calls that are not direct are outside the experiment.
    buck.targets(&["//:", "-c", "foo.bar=1", DIRECT]).await?;
    let record = buck.targets(&["//:"]).await?.invocation_record()?;
    assert!(!has_tag(&record, IMPLICIT_TAG));
    Ok(())
}

#[buck_test]
async fn test_master_switch_does_not_affect_the_guard(buck: &Buck) -> anyhow::Result<()> {
    for master in ["true", "false"] {
        buck.targets(&targets_args(&["-c", "foo.bar=1"])).await?;
        buck.targets(&targets_args(&[&format!(
            "--setting=agent_interactive.enabled={master}"
        )]))
        .await
        .expect_failure()
        .assert_stderr_matches(ADVICE);
    }
    Ok(())
}

#[buck_test]
async fn test_disabled_and_non_direct_calls_keep_existing_behavior(
    buck: &Buck,
) -> anyhow::Result<()> {
    for args in [
        vec![DIRECT],
        // The master switch alone does not turn on this guard.
        vec!["--setting=agent_interactive.enabled=true", DIRECT],
        [
            &ENABLED[..],
            &["--setting=agent_config_change.enabled=false", DIRECT],
        ]
        .concat(),
        ENABLED.to_vec(),
        [&ENABLED[..], &["--agent-context=direct_call=false"]].concat(),
    ] {
        for config in [&["-c", "foo.bar=1"][..], &[]] {
            let result = buck.targets(&[&["//:"], config, &args].concat()).await?;
            assert!(!result.stderr.contains("Buck advice"));
        }
    }
    Ok(())
}
