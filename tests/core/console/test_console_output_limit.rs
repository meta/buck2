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

const OUTPUT_LIMIT_EXCEEDED: &str = "(output limit exceeded)";

#[buck_test]
async fn test_noop(_buck: &Buck) {}

/// With a small output limit, action stderr should be truncated.
#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_output_limit_truncates_action_stderr(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_env("BUCK2_CONSOLE_OUTPUT_LIMIT", "10");
    let res = buck
        .build(&["--console=simplenotty", "-v5", "//:noisy1"])
        .await?;
    // The exceeded message should appear exactly once.
    assert_eq!(
        res.stderr.matches(OUTPUT_LIMIT_EXCEEDED).count(),
        1,
        "{}",
        res.stderr
    );
    // With a 10-byte limit, most of the output should be suppressed.
    assert!(
        res.stderr.matches("stderr line").count() <= 1,
        "{}",
        res.stderr
    );
    Ok(())
}

/// Without the env var, all output should be printed.
#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_output_limit_not_set_prints_all(buck: &Buck) -> anyhow::Result<()> {
    let res = buck
        .build(&["--console=simplenotty", "-v5", "//:noisy1"])
        .await?;
    assert!(
        !res.stderr.contains(OUTPUT_LIMIT_EXCEEDED),
        "{}",
        res.stderr
    );
    assert_eq!(
        res.stderr.matches("stderr line").count(),
        10,
        "{}",
        res.stderr
    );
    Ok(())
}

/// The limit is global: building two noisy targets should still only
/// print the exceeded message once.
#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_output_limit_global_across_actions(buck: &mut Buck) -> anyhow::Result<()> {
    buck.set_env("BUCK2_CONSOLE_OUTPUT_LIMIT", "10");
    let res = buck
        .build(&["--console=simplenotty", "-v5", "//:noisy1", "//:noisy2"])
        .await?;
    assert_eq!(
        res.stderr.matches(OUTPUT_LIMIT_EXCEEDED).count(),
        1,
        "{}",
        res.stderr
    );
    Ok(())
}

/// With a small output limit, test result output should be truncated.
#[buck_test(skip_for_os = ["darwin", "windows"])]
async fn test_output_limit_truncates_test_output(buck: &mut Buck) {
    buck.set_env("BUCK2_CONSOLE_OUTPUT_LIMIT", "10");
    // The test rule exits with 1 so that test result details (stderr) are
    // always printed, letting us verify the output limit applies to them.
    let res = buck
        .test(&["--console=simplenotty", "//:noisy_test"])
        .await
        .expect_failure();
    assert_eq!(
        res.stderr.matches(OUTPUT_LIMIT_EXCEEDED).count(),
        1,
        "{}",
        res.stderr
    );
    assert!(
        res.stderr.matches("test output line").count() <= 1,
        "{}",
        res.stderr
    );
}
