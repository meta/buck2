/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::BuckCommand;
use buck2_e2e::prelude::*;
use serde_json::Value;

const ENABLED: &str = "--setting=agent_recursive_patterns.enabled=true";
const DISABLED: &str = "--setting=agent_recursive_patterns.enabled=false";
const DIRECT: &str = "--agent-context=direct_call=true";
const ACK: &str =
    "--agent-context=advice_ack=recursive_patterns,reason=Validate the whole package tree";
const ADVICE: &str = r"Buck advice \[recursive_patterns\]";
const PREFIX: &str = "agent_interactive.recursive_patterns.";

fn invoke<'a>(buck: &'a Buck, command: &str, args: &[&str]) -> BuckCommand<'a> {
    match command {
        "build" => buck.build(args),
        "test" => buck.test(args),
        _ => unreachable!(),
    }
}

fn has_tag(record: &Value, suffix: &str) -> bool {
    record["tags"]
        .as_array()
        .unwrap()
        .contains(&format!("{PREFIX}{suffix}").into())
}

#[buck_test(write_invocation_record = true)]
async fn test_recursive_forms_block_before_expansion(buck: &Buck) -> anyhow::Result<()> {
    // Acknowledging reaches this failure; the guard must not load even the first matching package.
    std::fs::write(
        buck.cwd().join("sub/TARGETS.fixture"),
        "fail('recursive package loaded')\n",
    )?;
    for command in ["build", "test"] {
        for pattern in [
            "...",
            "//...",
            "root//...",
            "sub/...",
            "alias//sub/...",
            "//sub/...?root//:modifier",
            "@outer.args",
        ] {
            let blocked = invoke(buck, command, &[pattern, ENABLED, DIRECT])
                .await
                .expect_failure();
            blocked.assert_stderr_matches(ADVICE);
            assert!(!blocked.stderr.contains("recursive package loaded"));
            assert!(
                blocked
                    .stderr
                    .contains("does not require user confirmation")
            );
            let record = blocked.invocation_record()?;
            for tag in ["enabled", "detected", "block"] {
                assert!(has_tag(&record, tag), "{command} {pattern}: {tag}");
            }
            assert!(
                record.single_error()["tags"]
                    .as_array()
                    .unwrap()
                    .contains(&"AGENT_ADVICE_BLOCKED".into())
            );
        }
        invoke(buck, command, &["//sub/...", ENABLED, DIRECT, ACK])
            .await
            .expect_failure()
            .assert_stderr_matches("recursive package loaded");
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_ack_is_specific_nonempty_and_per_invocation(buck: &Buck) -> anyhow::Result<()> {
    for command in ["build", "test"] {
        for context in [
            "advice_ack=build,reason=Validate everything",
            "advice_ack=recursive_patterns",
            "advice_ack=recursive_patterns,reason=   ",
            "advice_ack=recursive_patterns,reason=old,reason=",
            "reason=Validate everything",
        ] {
            invoke(
                buck,
                command,
                &[
                    "//sub/...",
                    ENABLED,
                    DIRECT,
                    &format!("--agent-context={context}"),
                ],
            )
            .await
            .expect_failure()
            .assert_stderr_matches(ADVICE);
        }
        let result = invoke(
            buck,
            command,
            &[
                "//sub/...",
                ENABLED,
                DIRECT,
                "--agent-context=advice_ack=build",
                ACK,
            ],
        )
        .await?;
        let record = result.invocation_record()?;
        assert!(
            has_tag(&record, "enabled") && has_tag(&record, "detected") && has_tag(&record, "ack")
        );
        assert!(!has_tag(&record, "block"));
        invoke(buck, command, &["//sub/...", ENABLED, DIRECT])
            .await
            .expect_failure()
            .assert_stderr_matches(ADVICE);
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_narrowed_calls_and_control_arm_are_observable(buck: &Buck) -> anyhow::Result<()> {
    for command in ["build", "test"] {
        for target in ["//:ok", "//sub:"] {
            let result = invoke(
                buck,
                command,
                &[
                    target,
                    ENABLED,
                    DIRECT,
                    "--agent-context=reason=Inspect foo/...",
                ],
            )
            .await?;
            let record = result.invocation_record()?;
            assert!(has_tag(&record, "enabled"));
            assert!(!has_tag(&record, "detected") && !has_tag(&record, "block"));
        }
        for args in [
            vec![DIRECT],
            vec![ENABLED, DISABLED, DIRECT],
            vec!["--setting=agent_interactive.enabled=true", DIRECT],
        ] {
            let result = invoke(buck, command, &[&["//sub/..."][..], &args].concat()).await?;
            let record = result.invocation_record()?;
            assert!(has_tag(&record, "disabled") && has_tag(&record, "detected"));
            assert!(
                !has_tag(&record, "enabled")
                    && !has_tag(&record, "block")
                    && !has_tag(&record, "ack")
            );
        }
        for master in ["true", "false"] {
            invoke(
                buck,
                command,
                &[
                    "//sub/...",
                    ENABLED,
                    DIRECT,
                    &format!("--setting=agent_interactive.enabled={master}"),
                ],
            )
            .await
            .expect_failure()
            .assert_stderr_matches(ADVICE);
        }
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_non_direct_calls_and_other_commands_are_unchanged(buck: &Buck) -> anyhow::Result<()> {
    for command in ["build", "test"] {
        for args in [
            vec![ENABLED],
            vec![ENABLED, DIRECT, "--agent-context=direct_call=false"],
        ] {
            let result = invoke(buck, command, &[&["//sub/..."][..], &args].concat()).await?;
            let record = result.invocation_record()?;
            assert!(!has_tag(&record, "enabled") && !has_tag(&record, "detected"));
        }
    }
    for result in [
        buck.targets(&["//...", ENABLED, DIRECT]).await?,
        buck.uquery(&["//...", ENABLED, DIRECT]).await?,
        buck.run(&["//sub/...", ENABLED, DIRECT, "--", "foo/..."])
            .await?,
    ] {
        assert!(!has_tag(&*result.invocation_record()?, "enabled"));
        assert!(!result.stderr.contains("Buck advice"));
    }
    Ok(())
}
