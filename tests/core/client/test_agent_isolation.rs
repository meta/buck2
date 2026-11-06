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
use buck2_e2e::utils::random_string;
use serde_json::Value;

const ENABLED: [&str; 2] = [
    "--setting=agent_interactive.enabled=true",
    "--setting=agent_interactive.new_isolation_dir_enabled=true",
];
const DIRECT: &str = "--agent-context=direct_call=true";

/// What `buck2 status` reports.
async fn status(buck: &Buck) -> anyhow::Result<Value> {
    Ok(serde_json::from_str(&buck.status(&[]).await?.stdout)?)
}

fn has_tag(record: &Value, tag: &str) -> bool {
    record["tags"].as_array().unwrap().contains(&tag.into())
}

/// A fresh isolation directory name.
fn new_isolation_prefix() -> String {
    random_string()[..32].to_owned()
}

#[buck_test]
async fn test_client_settings_reuse_existing_daemon(buck: &Buck) -> anyhow::Result<()> {
    buck.audit_config(&[]).await?;
    let daemon_id = status(buck).await?["daemon_constraints"]["daemon_id"]
        .as_str()
        .unwrap()
        .to_owned();
    for args in [[&ENABLED[..], &[DIRECT]].concat(), vec![DIRECT]] {
        buck.audit_config(&args).await?;
        assert_eq!(
            status(buck).await?["daemon_constraints"]["daemon_id"],
            daemon_id
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_new_isolation_is_blocked_until_acknowledged(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_isolation_prefix(new_isolation_prefix());
    // Status may create the daemon directory, but that does not initialize an isolation.
    assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    for context in [
        "reason=Inspect config",
        "advice_ack=killall,reason=Inspect config",
        "advice_ack=clean,reason=Inspect config",
        "advice_ack=new_isolation_dir",
        "advice_ack=new_isolation_dir,reason=   ",
    ] {
        let context_arg = format!("--agent-context={context}");
        let result = buck
            .audit_config(&[&ENABLED[..], &[DIRECT, &context_arg]].concat())
            .await
            .expect_failure();
        result
            .assert_exit_code(ExitCode::UserError)
            .assert_stderr_matches(r"Buck advice \[new_isolation_dir\]");
        assert!(!result.stderr.contains("Starting new buck2 daemon"));
        assert!(!result.stderr.contains("Failed to connect to buck daemon"));
        assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
        let record = result.invocation_record()?;
        assert!(has_tag(
            &record,
            "agent_interactive.new_isolation_dir.block"
        ));
        assert!(has_tag(
            &record,
            "agent_interactive.new_isolation_dir.enabled"
        ));
        assert!(has_tag(record.single_error(), "AGENT_ADVICE_BLOCKED"));
    }

    let result = buck
        .audit_config(
            &[
                &ENABLED[..],
                &[
                    DIRECT,
                    "--agent-context=advice_ack=new_isolation_dir,reason=Use isolated test state",
                ],
            ]
            .concat(),
        )
        .await?;
    assert!(has_tag(
        &*result.invocation_record()?,
        "agent_interactive.new_isolation_dir.ack"
    ));
    let before = status(buck).await?["process_info"]["pid"].as_u64().unwrap();
    let reused = buck
        .audit_config(&[&ENABLED[..], &[DIRECT]].concat())
        .await?;
    assert_eq!(status(buck).await?["process_info"]["pid"], before);
    assert!(!has_tag(
        &*reused.invocation_record()?,
        "agent_interactive.new_isolation_dir.ack"
    ));
    buck.kill().await?;
    // Restarting a previously initialized isolation also reuses its stored state.
    buck.audit_config(&[&ENABLED[..], &[DIRECT]].concat())
        .await?;
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_disabled_direct_new_isolation_records_its_state(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    buck.set_isolation_prefix(new_isolation_prefix());
    let result = buck.audit_config(&[DIRECT]).await?;
    assert!(!result.stderr.contains("Buck advice"));
    assert!(has_tag(
        &*result.invocation_record()?,
        "agent_interactive.new_isolation_dir.disabled"
    ));

    // Reusing the isolation's daemon does not start a new isolation.
    let reused = buck.audit_config(&[DIRECT]).await?;
    assert!(!has_tag(
        &*reused.invocation_record()?,
        "agent_interactive.new_isolation_dir.disabled"
    ));
    Ok(())
}

#[buck_test]
async fn test_default_isolation_needs_no_ack(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_isolation_prefix("v2");
    let result = buck
        .audit_config(&[&ENABLED[..], &[DIRECT]].concat())
        .await?;
    assert!(!result.stderr.contains("Buck advice"));
    Ok(())
}

#[buck_test]
async fn test_disabled_and_non_direct_isolation_keep_existing_behavior(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    buck.set_isolation_prefix(new_isolation_prefix());
    for args in [
        vec![DIRECT],
        [
            &ENABLED[..],
            &[
                "--setting=agent_interactive.new_isolation_dir_enabled=false",
                DIRECT,
            ],
        ]
        .concat(),
        ENABLED.to_vec(),
        [&ENABLED[..], &["--agent-context=direct_call=false"]].concat(),
    ] {
        let result = buck.audit_config(&args).await?;
        assert!(!result.stderr.contains("Buck advice"));
        buck.clean(&[]).await?;
    }
    Ok(())
}

#[buck_test]
async fn test_environment_isolation_is_guarded(buck: &mut Buck) {
    let isolation = new_isolation_prefix();
    // The test's commands run without `--isolation-dir` until the end, where cleaning up uses the
    // isolation the environment chose.
    let result = buck
        .audit_config(&[&ENABLED[..], &[DIRECT]].concat())
        .env("BUCK_ISOLATION_DIR", &isolation)
        .await;
    buck.set_isolation_prefix(isolation);
    result
        .expect_failure()
        .assert_stderr_matches(r"Buck advice \[new_isolation_dir\]");
}
