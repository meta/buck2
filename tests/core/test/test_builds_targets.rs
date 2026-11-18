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

async fn check_build_and_skip_override(
    buck: &Buck,
    target: &str,
    skip_flag: &str,
    config_args: &[&str],
    failure_category: &str,
) -> anyhow::Result<()> {
    let mut args = vec![target];
    args.extend(config_args);
    let result = buck.test(&args).await.expect_failure();
    assert!(result.stderr.contains(failure_category));

    args.push(skip_flag);
    buck.test(&args).await?;
    Ok(())
}

#[buck_test]
async fn test_builds_targets_from_command_line_config(buck: &Buck) -> anyhow::Result<()> {
    check_build_and_skip_override(
        buck,
        "//:failing_default_info",
        "--skip-default-info",
        &["-c", "buck2.test_builds_targets=true"],
        "failing_default_info",
    )
    .await?;
    check_build_and_skip_override(
        buck,
        "//:failing_run_info",
        "--skip-run-info",
        &["-c", "buck2.test_builds_targets=true"],
        "failing_run_info",
    )
    .await
}

#[buck_test]
async fn test_explicit_build_flags_override_disabled_config(buck: &Buck) {
    let default_result = buck
        .test(&[
            "//:failing_default_info",
            "-c",
            "buck2.test_builds_targets=false",
            "--build-default-info",
        ])
        .await
        .expect_failure();
    assert!(default_result.stderr.contains("failing_default_info"));

    let run_result = buck
        .test(&[
            "//:failing_run_info",
            "-c",
            "buck2.test_builds_targets=false",
            "--build-run-info",
        ])
        .await
        .expect_failure();
    assert!(run_result.stderr.contains("failing_run_info"));
}

#[buck_test]
async fn test_builds_targets_from_included_config_file(buck: &Buck) -> anyhow::Result<()> {
    let config_file = buck.cwd().join("test_builds_targets_include.bcfg");

    check_build_and_skip_override(
        buck,
        "//:failing_default_info",
        "--skip-default-info",
        &["--config-file", &config_file.to_string_lossy()],
        "failing_default_info",
    )
    .await
}
