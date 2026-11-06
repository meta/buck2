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
use regex::NoExpand;
use regex::Regex;

#[buck_test]
async fn test_soft_error(buck: &Buck) {
    buck.targets(&[":"])
        .await
        .expect_failure()
        .assert_stderr_matches("starlark_raised_soft_error.*Will be reported");
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_soft_error_is_silent(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&[":", "no_stack:"]).await?;
    assert!(!res.stderr.contains("starlark_raised_soft_error"));
    assert!(!res.stderr.contains("starlark_no_stack_soft_error"));
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_soft_error_no_stack(buck: &Buck) -> anyhow::Result<()> {
    buck.targets(&[":"]).await?;
    let log = buck.log(&["show"]).await?;
    assert!(log.stdout.contains("Traceback"));

    buck.targets(&["no_stack:"]).await?;
    let log = buck.log(&["show"]).await?;
    assert!(!log.stdout.contains("Traceback"));
    Ok(())
}

#[buck_test]
async fn test_deprecation(buck: &Buck) {
    buck.targets(&["deprecation:"])
        .await
        .expect_failure()
        .assert_stderr_matches("starlark_deprecation_example.*Going away");
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_deprecation_is_silent(buck: &Buck) -> anyhow::Result<()> {
    let res = buck.targets(&["deprecation:"]).await?;
    assert!(!res.stderr.contains("starlark_deprecation_example"));
    let log = buck.log(&["show"]).await?;
    assert!(log.stdout.contains("starlark_deprecation_example"));
    Ok(())
}

#[buck_test(env = [("BUCK2_HARD_ERROR", "false")])]
async fn test_deprecation_start_must_be_a_day(buck: &Buck) {
    buck.targets(&["deprecation_bad_start:"])
        .await
        .expect_failure()
        .assert_stderr_matches("yesterday");
}

/// `stderr` with the configuration hashes of target labels zeroed.
fn zero_cfg_hashes(stderr: &str) -> String {
    Regex::new(r"#[a-f0-9]*\)")
        .unwrap()
        .replace_all(stderr, NoExpand("#00000000)"))
        .into_owned()
}

#[buck_test(
    // windows errors are slightly different, just skip for now
    skip_for_os = ["windows"],
)]
async fn test_configured_graph_deps_collapsed_in_errors(buck: &Buck) {
    let out = buck
        .cquery(&[
            "//deps_collapsed:top",
            "-v=0",
            "--console=none",
            "-c",
            "build.execution_platforms=root//deps_collapsed:exec_platforms",
        ])
        .await
        .expect_failure();
    let stderr = zero_cfg_hashes(&out.stderr);
    golden(&stderr, "deps_collapsed/expected.golden.out");
}

#[buck_test(
    // windows errors are slightly different, just skip for now
    skip_for_os = ["windows"],
)]
async fn test_configured_graph_deps_collapsed_in_errors_2(buck: &Buck) {
    let out = buck
        .cquery(&[
            "//deps_collapsed:top",
            "-v=0",
            "--console=none",
            "-c",
            "build.execution_platforms=root//deps_collapsed:exec_platforms",
            "-c",
            "core_test_errors.broken_select_in_toolchain=1",
        ])
        .await
        .expect_failure();
    let stderr = zero_cfg_hashes(&out.stderr);
    golden(&stderr, "deps_collapsed/expected_2.golden.out");
}
