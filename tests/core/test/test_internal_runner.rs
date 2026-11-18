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

use buck2_e2e::ansi::remove_ansi_escape_sequences;
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

/// Runs both tests on two local slots and returns whether they ran at the same time.
async fn tests_overlapped(
    buck: &Buck,
    tmp_path: &Path,
    targets: [&str; 2],
) -> anyhow::Result<bool> {
    let lock_dir = tmp_path.join(targets[0].trim_start_matches(':'));
    std::fs::create_dir(&lock_dir)?;
    let env = format!("LOCK_DIR={}", lock_dir.display());
    buck.test(&[targets[0], targets[1], "-j", "2", "--", "--env", &env])
        .await?;
    Ok(lock_dir.join("overlapped").exists())
}

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_internal_test_executor_weight(
    buck: &mut Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    use_internal_test_executor(buck);
    assert!(
        tests_overlapped(buck, tmp_path, [":unweighted_a", ":unweighted_b"]).await?,
        "Two tests of one slot each should share two slots"
    );
    assert!(
        !tests_overlapped(buck, tmp_path, [":weighted_a", ":weighted_b"]).await?,
        "Two tests labelled `buck2:weight=2` should not share two slots"
    );
    Ok(())
}

#[buck_test(env = [("BUCK2_ALLOW_INTERNAL_TEST_RUNNER_DO_NOT_USE", "1")])]
async fn test_internal_test_executor_invalid_weight(buck: &mut Buck) {
    use_internal_test_executor(buck);
    let failure = buck
        .test(&[":invalid_weight", ":trivial_pass"])
        .await
        .expect_failure();
    failure.assert_stderr_matches(
        "Test `root//:invalid_weight`[\\s\\S]*Invalid test label `buck2:weight=many`",
    );
    // The other test still runs and reports its result.
    assert!(
        remove_ansi_escape_sequences(&failure.stderr)
            .contains("Pass 1. Fail 0. Timeout 0. Fatal 1."),
        "{}",
        failure.stderr
    );
}
