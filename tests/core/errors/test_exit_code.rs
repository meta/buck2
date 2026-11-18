/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::time::Duration;

use buck2_e2e::prelude::*;

#[buck_test]
async fn test_exit_code_build_success(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&[":build_success"]).await?;
    assert_eq!(result.exit_code(), Some(ExitCode::Success as i32));
    Ok(())
}

#[buck_test]
async fn test_exit_code_build_fail(buck: &Buck) {
    // TODO(nga): check non-existing command
    // TODO(nga): check local vs RE
    buck.build(&[":build_fail"])
        .await
        .expect_failure()
        .assert_exit_code(ExitCode::UserError);
}

// Deliberately cause a daemon connection failure.
// This test case spawns a loose daemon that we can't connect to. On windows
// this loose daemon will keep holding onto buck-out files after test case finishes
// and prevent other processes from changing them, so set a termination timeout
// of 20 seconds so that this loose daemon gets killed before test case finishes.
#[buck_test(
    allow_daemon_panics = true,
    env = [("BUCK2_TEST_FAIL_BUCKD_AUTH", "true"), ("BUCK2_TERMINATE_AFTER", "15")],
)]
async fn test_exit_code_fail_buckd_auth_for_unknown_reason(buck: &Buck) {
    buck.build(&[":build_success"])
        .await
        .expect_failure()
        .assert_exit_code(ExitCode::ConnectError);
    tokio::time::sleep(Duration::from_secs(20)).await; // Makes sure the daemon terminates before test case finishes
}
