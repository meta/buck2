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

/// An empty test executor forces the internal test executor to be used.
fn use_internal_test_executor(buck: &mut Buck) {
    buck.set_env("BUCK2_TPX", "");
}

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_internal_test_executor(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    buck.test(&[":trivial_pass"]).await?;
    Ok(())
}

#[buck_test(env = [
    ("TEST_VAR", "BAD_VALUE"),
    ("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1"),
])]
async fn test_internal_test_executor_env(buck: &mut Buck) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    buck.test(&[":check_env", "--", "--env", "TEST_VAR=TEST_VALUE"])
        .await?;
    Ok(())
}

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_internal_test_executor_timeout(buck: &mut Buck) {
    use_internal_test_executor(buck);
    buck.test(&[":timeout", "--", "--timeout", "1"])
        .await
        .expect_failure()
        .assert_stderr_matches("Timeout: ");
}
