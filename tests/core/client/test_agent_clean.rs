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
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use serde_json::Value;

const ENABLED: [&str; 2] = [
    "--setting=agent_interactive.enabled=true",
    "--setting=agent_interactive.clean_enabled=true",
];
const DIRECT: &str = "--agent-context=direct_call=true";

/// The pid of the running daemon, as `buck2 status` reports it.
async fn daemon_pid(buck: &Buck) -> anyhow::Result<u64> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    status["process_info"]["pid"]
        .as_u64()
        .with_context(|| format!("No pid in the status: {status}"))
}

/// A fresh isolation directory name.
fn new_isolation_prefix() -> String {
    random_string()[..32].to_owned()
}

#[buck_test(write_invocation_record = true)]
async fn test_block_preserves_daemon_and_outputs(buck: &mut Buck) -> anyhow::Result<()> {
    let isolation_prefix = new_isolation_prefix();
    buck.set_isolation_prefix(&isolation_prefix);
    buck.audit_config(&ENABLED).await?;
    let before = daemon_pid(buck).await?;
    let output = buck
        .cwd()
        .join("buck-out")
        .join(&isolation_prefix)
        .join("output.txt");
    std::fs::write(&output, "preserve this output")?;

    for args in [&[][..], &["--background"]] {
        let result = buck
            .clean(&[args, &ENABLED, &[DIRECT]].concat())
            .await
            .expect_failure();
        result.assert_stderr_matches(r"Buck advice \[clean\]");
        assert_eq!(std::fs::read_to_string(&output)?, "preserve this output");
        assert_eq!(daemon_pid(buck).await?, before);
        let record = result.invocation_record()?;
        assert!(
            record["tags"]
                .as_array()
                .unwrap()
                .contains(&"agent_interactive.clean.block".into())
        );
        assert!(
            record["tags"]
                .as_array()
                .unwrap()
                .contains(&"agent_interactive.clean.enabled".into())
        );
        assert!(
            record.single_error()["tags"]
                .as_array()
                .unwrap()
                .contains(&"AGENT_ADVICE_BLOCKED".into())
        );
    }
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_clean_requires_its_own_ack_and_nonempty_reason(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    let isolation_prefix = new_isolation_prefix();
    buck.set_isolation_prefix(&isolation_prefix);
    buck.audit_config(&[]).await?;
    let output = buck
        .cwd()
        .join("buck-out")
        .join(&isolation_prefix)
        .join("output.txt");
    std::fs::write(&output, "remove only after acknowledgement")?;
    for context in [
        "advice_ack=killall,reason=Reset state",
        "advice_ack=new_isolation_dir,reason=Reset state",
        "advice_ack=clean",
        "advice_ack=clean,reason=   ",
        "reason=Reset state",
    ] {
        buck.clean(
            &[
                &ENABLED[..],
                &[DIRECT, &format!("--agent-context={context}")],
            ]
            .concat(),
        )
        .await
        .expect_failure()
        .assert_stderr_matches(r"Buck advice \[clean\]");
        assert!(output.exists());
    }

    let result = buck
        .clean(
            &[
                &ENABLED[..],
                &[
                    DIRECT,
                    "--agent-context=advice_ack=clean,reason=Reset state for this test",
                ],
            ]
            .concat(),
        )
        .await?;
    assert!(!output.exists());
    assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    assert!(
        result.invocation_record()?["tags"]
            .as_array()
            .unwrap()
            .contains(&"agent_interactive.clean.ack".into())
    );
    Ok(())
}

#[buck_test(write_invocation_record = true)]
async fn test_disabled_direct_clean_records_its_state(buck: &mut Buck) -> anyhow::Result<()> {
    let isolation_prefix = new_isolation_prefix();
    buck.set_isolation_prefix(&isolation_prefix);
    buck.audit_config(&[]).await?;
    let output = buck
        .cwd()
        .join("buck-out")
        .join(&isolation_prefix)
        .join("output.txt");
    std::fs::write(&output, "remove this output")?;
    let result = buck.clean(&[DIRECT]).await?;
    assert!(!result.stderr.contains("Buck advice"));
    assert!(!output.exists());
    assert!(
        result.invocation_record()?["tags"]
            .as_array()
            .unwrap()
            .contains(&"agent_interactive.clean.disabled".into())
    );

    // A dry run, a stale cleanup, or a call that is not direct is not recorded.
    buck.audit_config(&[]).await?;
    for args in [&["--dry-run", DIRECT][..], &["--stale=1d", DIRECT], &[]] {
        let result = buck.clean(args).await?;
        assert!(
            !result.invocation_record()?["tags"]
                .as_array()
                .unwrap()
                .contains(&"agent_interactive.clean.disabled".into()),
            "{args:?}"
        );
    }
    Ok(())
}

#[buck_test]
async fn test_dry_run_and_stale_do_not_require_full_clean_ack(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    let isolation_prefix = new_isolation_prefix();
    buck.set_isolation_prefix(&isolation_prefix);
    buck.audit_config(&ENABLED).await?;
    let before = daemon_pid(buck).await?;
    let output = buck
        .cwd()
        .join("buck-out")
        .join(&isolation_prefix)
        .join("output.txt");
    std::fs::write(&output, "preserve this output")?;
    for args in [&[][..], &["--background"], &["--stale=1d"]] {
        let result = buck
            .clean(&[&["--dry-run"], args, &ENABLED, &[DIRECT]].concat())
            .await?;
        assert!(!result.stderr.contains("Buck advice"));
        assert_eq!(std::fs::read_to_string(&output)?, "preserve this output");
        assert_eq!(daemon_pid(buck).await?, before);
    }
    Ok(())
}

#[buck_test]
async fn test_disabled_and_non_direct_clean_keep_existing_behavior(
    buck: &mut Buck,
) -> anyhow::Result<()> {
    let isolation_prefix = new_isolation_prefix();
    buck.set_isolation_prefix(&isolation_prefix);
    for args in [
        vec![DIRECT],
        [
            &ENABLED[..],
            &["--setting=agent_interactive.clean_enabled=false", DIRECT],
        ]
        .concat(),
        ENABLED.to_vec(),
        [&ENABLED[..], &["--agent-context=direct_call=false"]].concat(),
    ] {
        buck.audit_config(&[]).await?;
        let output = buck
            .cwd()
            .join("buck-out")
            .join(&isolation_prefix)
            .join("output.txt");
        std::fs::write(&output, "remove this output")?;
        let result = buck.clean(&args).await?;
        assert!(!result.stderr.contains("Buck advice"));
        assert!(!output.exists());
        assert!(buck.status(&[]).await?.stderr.contains("no buckd running"));
    }
    Ok(())
}

#[buck_test]
async fn test_clean_guard_works_with_invalid_build_config(buck: &Buck) -> anyhow::Result<()> {
    std::fs::write(buck.cwd().join(".buckconfig"), "invalid buckconfig")?;
    buck.clean(&[&ENABLED[..], &[DIRECT]].concat())
        .await
        .expect_failure()
        .assert_stderr_matches(r"Buck advice \[clean\]");
    Ok(())
}
