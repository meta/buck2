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

/// On windows, we get an error of form
/// "The process cannot access the file because it is being used by another process"
/// when trying to kill the daemon with sqlite states enabled. This is most
/// likely because we don't kill all child processes of the daemon and so the sqlite process
/// is still running and accessing the sqlite db file when being killed. Given this is a
/// pre-existing issue, we disable sqlite state on windows for now.
fn sqlite_state_config() -> Vec<(&'static str, &'static str, &'static str)> {
    if cfg!(windows) {
        vec![
            ("buck2", "sqlite_materializer_state", "false"),
            ("buck2", "sqlite_incremental_state", "false"),
        ]
    } else {
        Vec::new()
    }
}

#[buck_test(
    extra_buck_config = sqlite_state_config(),
    env = [("BUCK2_TEST_FAIL_BUCKD_AUTH", "true"), ("BUCK2_SHARE_DAEMON", "false")],
)]
async fn test_kill_error(buck: &Buck) -> anyhow::Result<()> {
    // Performing a build should fail, since we will not be able to authenticate to the
    // buck daemon
    buck.build(&["//:abc"])
        .await
        .expect_failure()
        .assert_stderr_matches("injected auth error");

    // Kill should succeed, even though we cannot authenticate to the daemon
    buck.kill().await?;
    Ok(())
}

#[buck_test(
    extra_buck_config = sqlite_state_config(),
    env = [("BUCK2_TEST_FAIL_BUCKD_AUTH", "true")],
)]
async fn test_clean_error(buck: &Buck) -> anyhow::Result<()> {
    // Performing a build should fail, since we will not be able to authenticate to the
    // buck daemon
    buck.build(&["//:abc"])
        .await
        .expect_failure()
        .assert_stderr_matches("injected auth error");

    // Clean should succeed, even though we cannot authenticate to the daemon
    buck.clean(&[]).await?;
    Ok(())
}
