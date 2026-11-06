/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use serde_json::Value;

async fn scoped_killall(
    buck: &Buck,
    isolation_prefix: &str,
    args: &[&str],
) -> Result<BuckOutput, buck2_e2e::BuckError> {
    // Keep even a broken guard confined to this test's daemon.
    assert!(!isolation_prefix.is_empty());
    buck.command(
        &[
            &["killall", "--repo", "--in-isolation-dir", isolation_prefix],
            args,
        ]
        .concat(),
    )
    .await
}

/// The pid of the running daemon, as `buck2 status` reports it.
async fn daemon_pid(buck: &Buck) -> anyhow::Result<u64> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    status["process_info"]["pid"]
        .as_u64()
        .with_context(|| format!("No pid in the status: {status}"))
}

fn has_tag(record: &Value, tag: &str) -> bool {
    record["tags"].as_array().unwrap().contains(&tag.into())
}

/// A fresh isolation directory name, which the test's commands use from then on.
fn set_new_isolation_prefix(buck: &mut Buck) -> String {
    let isolation_prefix = random_string()[..32].to_owned();
    buck.set_isolation_prefix(&isolation_prefix);
    isolation_prefix
}

#[buck_test(write_invocation_record = true)]
async fn test_block_preserves_daemon_and_records_advice(buck: &mut Buck) -> anyhow::Result<()> {
    let isolation_prefix = set_new_isolation_prefix(buck);
    buck.audit_config(&[]).await?;
    let before = daemon_pid(buck).await?;
    let result = scoped_killall(
        buck,
        &isolation_prefix,
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=true",
        ],
    )
    .await
    .expect_failure();
    result.assert_stderr_matches(r"Buck advice \[killall\]");
    assert_eq!(result.stdout, "");
    assert!(result.stderr.contains("No processes have been killed"));
    assert!(result.stderr.contains("advice_ack=killall"));
    assert!(result.stderr.contains("reason="));
    assert_eq!(daemon_pid(buck).await?, before);
    let record = result.invocation_record()?;
    assert_eq!(record["agent_direct_call"], true);
    assert!(has_tag(&record, "agent_interactive.killall.block"));
    assert!(has_tag(&record, "agent_interactive.killall.enabled"));
    assert!(has_tag(record.single_error(), "AGENT_ADVICE_BLOCKED"));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_acknowledgement_needs_exact_id_and_nonempty_reason(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    let isolation_prefix = set_new_isolation_prefix(buck);
    for context in [
        "advice_ack=build_intent,reason=Restart test daemon",
        "advice_ack=killall",
        "advice_ack=killall,reason=   ",
        "advice_ack=killall,reason=old,reason=",
        "reason=Restart test daemon",
    ] {
        scoped_killall(
            buck,
            &isolation_prefix,
            &[
                "--setting=agent_interactive.enabled=true",
                "--setting=agent_interactive.killall_enabled=true",
                "--agent-context=direct_call=true",
                &format!("--agent-context={context}"),
            ],
        )
        .await
        .expect_failure()
        .assert_stderr_matches(r"Buck advice \[killall\]");
    }
    assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));

    buck.audit_config(&[]).await?;
    let result = scoped_killall(
        buck,
        &isolation_prefix,
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=true,advice_ack=killall,reason=Restart test daemon",
        ],
    )
    .await?;
    assert!(!result.stderr.contains("Buck advice"));
    assert!(has_tag(
        &*result.invocation_record()?,
        "agent_interactive.killall.ack"
    ));
    assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_disabled_direct_killall_records_its_state(buck: &mut Buck) -> anyhow::Result<()> {
    let isolation_prefix = set_new_isolation_prefix(buck);
    buck.audit_config(&[]).await?;
    let result = scoped_killall(
        buck,
        &isolation_prefix,
        &["--agent-context=direct_call=true"],
    )
    .await?;
    assert!(!result.stderr.contains("Buck advice"));
    assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    assert!(has_tag(
        &*result.invocation_record()?,
        "agent_interactive.killall.disabled"
    ));

    // Calls that are not direct are outside the experiment.
    buck.audit_config(&[]).await?;
    let result = scoped_killall(buck, &isolation_prefix, &[]).await?;
    assert!(!has_tag(
        &*result.invocation_record()?,
        "agent_interactive.killall.disabled"
    ));
    Ok(())
}

#[buck_test]
async fn test_disabled_and_non_direct_calls_keep_existing_behavior(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    let isolation_prefix = set_new_isolation_prefix(buck);
    for args in [
        &["--agent-context=direct_call=true"][..],
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=false",
            "--agent-context=direct_call=true",
        ],
        &[
            "--setting=agent_interactive.enabled=false",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=true",
        ],
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
        ],
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=false",
        ],
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=invalid",
        ],
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=true,direct_call=false",
        ],
    ] {
        buck.audit_config(&[]).await?;
        let result = scoped_killall(buck, &isolation_prefix, args).await?;
        assert!(!result.stderr.contains("Buck advice"));
        assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    }
    Ok(())
}

#[buck_test]
async fn test_guard_works_without_valid_build_config(buck: &mut Buck) -> anyhow::Result<()> {
    let isolation_prefix = set_new_isolation_prefix(buck);
    std::fs::write(buck.cwd().join(".buckconfig"), "invalid buckconfig")?;
    scoped_killall(
        buck,
        &isolation_prefix,
        &[
            "--setting=agent_interactive.enabled=true",
            "--setting=agent_interactive.killall_enabled=true",
            "--agent-context=direct_call=true",
        ],
    )
    .await
    .expect_failure()
    .assert_stderr_matches(r"Buck advice \[killall\]");
    Ok(())
}

#[buck_test]
async fn test_outside_repository_keeps_existing_behavior(buck: &mut Buck) -> anyhow::Result<()> {
    let isolation_prefix = set_new_isolation_prefix(buck);
    buck.audit_config(&[]).await?;
    let before = daemon_pid(buck).await?;
    let outside = buck.cwd().join("outside");
    std::fs::create_dir(&outside)?;
    std::fs::File::create(outside.join(".buckroot"))?;
    assert!(!isolation_prefix.is_empty());
    let args = [
        "--in-isolation-dir",
        &isolation_prefix,
        "--setting=agent_interactive.enabled=true",
        "--setting=agent_interactive.killall_enabled=true",
        "--agent-context=direct_call=true",
    ];
    buck.command(&[&["killall", "--repo"][..], &args].concat())
        .rel_cwd("outside")
        .await
        .expect_failure()
        .assert_stderr_matches(r"`--repo` requires running from within a repository");
    assert_eq!(daemon_pid(buck).await?, before);
    let result = buck
        .command(&[&["killall"][..], &args].concat())
        .rel_cwd("outside")
        .await?;
    assert!(!result.stderr.contains("Buck advice"));
    assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    Ok(())
}
