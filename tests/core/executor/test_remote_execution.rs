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
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use buck2_e2e::BuckProcess;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::filter_events;
use buck2_e2e::utils::random_string;
use serde_json::Value;
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

/// The RE session the last command of `buck` ran in, as its event log announces it.
async fn re_session(buck: &Buck) -> anyhow::Result<Value> {
    filter_events(buck, &["Event", "data", "Instant", "data", "ReSession"])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("The command announced no RE session"))
}

/// The directory `session` names for its logs, checked to hold the RE client's log. A shared
/// daemon removes its logs when it exits.
fn logs_dir(session: &Value) -> anyhow::Result<PathBuf> {
    let logs_dir = PathBuf::from(
        session["logs_dir"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("The RE session names no log directory: {session}"))?,
    );
    anyhow::ensure!(
        logs_dir.join("REClientFolly.log").is_file(),
        "No RE client log in `{}`",
        logs_dir.display()
    );
    Ok(logs_dir)
}

async fn wait_for(path: &Path, build: &mut BuckProcess) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(300);
    while !path.exists() {
        anyhow::ensure!(
            build.try_wait().is_none(),
            "The build exited without creating `{}`",
            path.display()
        );
        anyhow::ensure!(
            Instant::now() < deadline,
            "Timed out waiting for `{}`",
            path.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

#[buck_test(meta_only = "remote execution")]
async fn test_re_session_names_its_log_directory(buck: &Buck) -> anyhow::Result<()> {
    buck.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await?;
    logs_dir(&re_session(buck).await?)?;
    Ok(())
}

/// Commands that run at the same time in two tenants of a shared daemon use one RE session,
/// which logs outside the buck-out of either.
#[buck_test(meta_only = "remote execution", skip_for_os = ["windows", "darwin"])]
async fn test_shared_daemon_tenants_share_an_re_session(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    buck.set_daemon_routing(true)?;
    let mut one = buck.clone();
    one.set_isolation_prefix("one");
    let mut two = buck.clone();
    two.set_isolation_prefix("two");

    // `:blocked` runs locally once `:simple` has run remotely, and keeps the first tenant's
    // command, and with it that command's hold on the session, until it is released.
    let started = tmp_path.join("started");
    let release = tmp_path.join("release");
    let gate = tmp_path.join("gate.bcfg");
    std::fs::write(
        &gate,
        format!(
            "[blocked]\nstarted = {}\nrelease = {}\n",
            started.display(),
            release.display()
        ),
    )?;
    let mut first = one
        .build(&[
            "root//:blocked",
            "--prefer-remote",
            "--no-remote-cache",
            "--config-file",
            &gate.to_string_lossy(),
        ])
        .spawn();
    let second_session = async {
        wait_for(&started, &mut first).await?;
        two.build(&["root//:simple", "--remote-only", "--no-remote-cache"])
            .await?;
        re_session(&two).await
    }
    .await;
    std::fs::write(&release, "")?;
    let first = first.wait().await;
    let sessions = async {
        let second_session = second_session?;
        first?;
        let first_session = re_session(&one).await?;
        let logs_dir = logs_dir(&first_session)?;
        anyhow::Ok((first_session, second_session, logs_dir))
    }
    .await;
    one.kill().await?;
    two.kill().await?;
    let (first_session, second_session, logs_dir) = sessions?;

    assert_eq!(first_session["session_id"], second_session["session_id"]);
    for tenant in ["one", "two"] {
        assert!(
            !logs_dir.starts_with(buck.cwd().join("buck-out").join(tenant)),
            "The shared daemon logs RE to `{}`, in a tenant's buck-out",
            logs_dir.display()
        );
    }
    Ok(())
}

#[buck_test(env = [("BUCK2_TEST_FAIL_CONNECT", "true")], meta_only = "remote execution")]
async fn test_re_connection_failure_no_retry(buck: &Buck) {
    let out = buck
        .build(&["root//:simple", "--remote-only", "--no-remote-cache"])
        .await
        .expect_failure();

    assert!(out.stderr.contains("Injected RE Connection error"));
    assert!(!out.stderr.contains("retrying after sleeping"));
}

#[buck_test(meta_only = "remote execution")]
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

#[buck_test(meta_only = "remote execution")]
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

#[buck_test(meta_only = "remote execution")]
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

#[buck_test(meta_only = "remote execution")]
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

#[buck_test(write_invocation_record = true, meta_only = "remote execution")]
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
