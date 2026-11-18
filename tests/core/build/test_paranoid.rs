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
use std::time::Duration;

use anyhow::Context;
use buck2_e2e::prelude::*;
use buck2_e2e::utils::random_string;
use serde_json::Value;

fn cache_buster() -> String {
    format!("test.cache_buster={}", random_string())
}

// FIXME(JakobDegen): Give these tests their own data dir, instead of sharing one

/// Currently, this is only supported in paranoid mode. This is a smaller issue
/// than it might seem, since it only enters the picture if RE claims, cancels
/// local, and *then* we need a fallback, which is fairly unlikely since most
/// of the work is done by then, and also if that fails, there's no reason to
/// expect deferred materialization won't fail later, which is a much bigger
/// (and much more irrecoverable) problem.
#[buck_test(
    data_dir = "execution_platforms",
    skip_for_os = ["windows"],
    env = [("BUCK2_TEST_FAIL_RE_DOWNLOADS", "true")],
)]
async fn test_paranoid_allows_fallback_after_re_failure(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//executor_race_tests:slower_locally_and_works_on_both";

    buck.build(&[target, "-c", &cache_buster()])
        .await
        .expect_failure()
        .assert_stderr_matches("Injected error");

    buck.build(&[target, "-c", &cache_buster()])
        .env("BUCK_PARANOID", "true")
        .await?;
    Ok(())
}

#[buck_test(data_dir = "execution_platforms", skip_for_os = ["windows"])]
async fn test_paranoid_ignores_preferences(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//executor_race_tests:fails_slow_on_re_works_locally_prefer_remote";

    buck.build(&[target, "-c", &cache_buster()])
        .await
        .expect_failure()
        .assert_stderr_matches("Remote command returned non-zero exit code 1");

    buck.build(&[target, "-c", &cache_buster()])
        .env("BUCK_PARANOID", "true")
        .await?;
    Ok(())
}

#[buck_test(data_dir = "execution_platforms", skip_for_os = ["windows"])]
async fn test_paranoid_forces_fallback_on_failure(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//executor_race_tests:slower_and_works_only_locally_local_not_preferred";

    // The local leg blocks until killed, so only the failing remote leg can finish.
    buck.build(&[
        target,
        "-c",
        &cache_buster(),
        "-c",
        "test.block_locally=true",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("Remote command returned non-zero exit code 1");

    buck.build(&[target, "-c", &cache_buster()])
        .env("BUCK_PARANOID", "true")
        .await?;
    Ok(())
}

#[buck_test(data_dir = "execution_platforms", skip_for_os = ["windows"])]
async fn test_paranoid_ignores_low_pass_filter(buck: &Buck) -> anyhow::Result<()> {
    let target = "root//executor_race_tests:fails_slow_on_re_works_locally_heavyweight";
    let low_pass_filter = "test.experimental_low_pass_filter=true";

    buck.build(&[target, "-c", &cache_buster(), "-c", low_pass_filter])
        .await
        .expect_failure()
        .assert_stderr_matches("Remote command returned non-zero exit code 1");

    buck.build(&[target, "-c", &cache_buster(), "-c", low_pass_filter])
        .env("BUCK_PARANOID", "true")
        .await?;
    Ok(())
}

async fn config(buck: &Buck) -> anyhow::Result<Value> {
    let status: Value = serde_json::from_str(&buck.status(&[]).await?.stdout)?;
    let config = status["daemon_constraints"]["daemon_startup_config"]
        .as_str()
        .context("No `daemon_startup_config` in `buck2 status`")?;
    eprintln!("{config}");
    Ok(serde_json::from_str(config)?)
}

#[buck_test(data_dir = "execution_platforms")]
async fn test_paranoid_enable_disable(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let paranoid_path = tmp_path.join("paranoid.info");

    // Start the daemon
    buck.build(&[])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;

    // Its not paranoid.
    assert_eq!(config(buck).await?["paranoid"], false);

    // Still not, we didn't run any commands, so no restart.
    buck.debug(&["paranoid", "enable"])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    assert_eq!(config(buck).await?["paranoid"], false);

    // Run a command, should restart and enable.
    buck.build(&[])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    assert_eq!(config(buck).await?["paranoid"], true);

    // Turn it off
    buck.debug(&["paranoid", "disable"])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    buck.build(&[])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    assert_eq!(config(buck).await?["paranoid"], false);

    // Turn it back on, but not for long.
    buck.debug(&["paranoid", "enable", "--ttl", "10s"])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    buck.build(&[])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    assert_eq!(config(buck).await?["paranoid"], true);

    // Wait until it turns off.
    tokio::time::sleep(Duration::from_secs(15)).await;
    buck.build(&[])
        .env("BUCK2_PARANOID_PATH", &paranoid_path)
        .await?;
    assert_eq!(config(buck).await?["paranoid"], false);
    Ok(())
}

#[buck_test(data_dir = "execution_platforms")]
async fn test_noop(_buck: &Buck) {}
