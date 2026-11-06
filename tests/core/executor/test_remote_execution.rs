/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use serde_json::json;

const RE_USE_CASE_STAGES: [&str; 4] = ["Queue", "WorkerDownload", "Execute", "WorkerUpload"];

async fn assert_re_use_case(buck: &Buck, expected_use_case: &str) -> anyhow::Result<()> {
    for action in RE_USE_CASE_STAGES {
        let use_cases = filter_events(
            buck,
            &[
                "Event",
                "data",
                "SpanStart",
                "data",
                "ExecutorStage",
                "stage",
                "Re",
                "stage",
                action,
                "use_case",
            ],
        )
        .await?;
        assert!(!use_cases.is_empty(), "No RE `{action}` stages found");
        assert!(
            use_cases
                .iter()
                .all(|use_case| use_case == expected_use_case),
            "{use_cases:?}"
        );
    }
    Ok(())
}

#[buck_test(env = [("BUCK2_TEST_FAIL_CONNECT", "true")])]
async fn test_re_connection_failure_no_retry(buck: &Buck) {
    let out = buck
        .build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await
        .expect_failure();

    assert!(out.stderr.contains("Injected RE Connection error"));
    assert!(!out.stderr.contains("retrying after sleeping"));
}

#[buck_test]
async fn test_re_use_case_override_with_arg(buck: &Buck) -> anyhow::Result<()> {
    // Make sure action is not cached
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;
    assert_re_use_case(buck, "buck2-testing").await?;
    // Change the target input
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    buck.build(&[
        "root//:simple",
        "--remote-only",
        "--no-remote-cache",
        "--config",
        "buck2_re_client.override_use_case=buck2-user",
    ])
    .await?;
    assert_re_use_case(buck, "buck2-user").await
}

#[buck_test]
async fn test_re_use_case_override_with_config(buck: &Buck) -> anyhow::Result<()> {
    // Make sure action is not cached
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;
    assert_re_use_case(buck, "buck2-testing").await?;
    // Change the target input
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    std::fs::write(
        buck.cwd().join(".buckconfig.local"),
        "[buck2_re_client]\noverride_use_case = buck2-user\n",
    )?;
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;
    assert_re_use_case(buck, "buck2-user").await
}

#[buck_test]
async fn test_re_use_case_override_with_external_config(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    // Make sure action is not cached
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;
    assert_re_use_case(buck, "buck2-testing").await?;
    // Change the target input
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    let config = tmp_path.join("external.bcfg");
    std::fs::write(
        &config,
        "[buck2_re_client]\noverride_use_case = buck2-user\n",
    )?;
    buck.build(&[
        "root//:simple",
        "--remote-only",
        "--no-remote-cache",
        "--config-file",
        &config.to_string_lossy(),
    ])
    .await?;
    assert_re_use_case(buck, "buck2-user").await
}

#[buck_test]
async fn test_re_use_case_override_with_external_config_source(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let temp = tmp_path.join("extra.bcfg");
    std::fs::write(&temp, "")?;
    // Make sure action is not cached
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", temp.as_os_str())
        .await?;
    assert_re_use_case(buck, "buck2-default").await?;
    // Change the target input
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    std::fs::write(&temp, "[buck2_re_client]\noverride_use_case = buck2-user\n")?;
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", temp.as_os_str())
        .await?;
    assert_re_use_case(buck, "buck2-user").await
}

#[buck_test(write_invocation_record = true)]
async fn test_re_over_quota_reports_exceeded_quotas(buck: &mut Buck) -> anyhow::Result<()> {
    let exceeded_quota = "worker_units:global:buck2-testing:linux-remote-execution";
    buck.set_env("BUCK2_TEST_INJECTED_RE_EXCEEDED_QUOTAS", exceeded_quota);
    // Make sure action is not cached
    std::fs::write(buck.cwd().join("input.txt"), random_string())?;
    let res = buck
        .build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;

    let over_quota = filter_events(
        buck,
        &[
            "Event",
            "data",
            "SpanStart",
            "data",
            "ExecutorStage",
            "stage",
            "Re",
            "stage",
            "QueueOverQuota",
            "exceeded_quotas",
        ],
    )
    .await?;
    assert_eq!(over_quota, [json!([exceeded_quota])]);
    assert_eq!(
        res.invocation_record()?["re_exceeded_quotas"],
        json!([exceeded_quota])
    );
    Ok(())
}
