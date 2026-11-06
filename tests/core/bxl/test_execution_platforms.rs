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
use buck2_e2e::utils::random_string;

#[buck_test(write_invocation_record = true)]
async fn test_bxl_exec_platform_dynamic_output(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//executor_fallback_tests/dynamic.bxl:test_dynamic_output",
            "-c",
            &format!("test.cache_buster={}", random_string()),
            "--local-only",
        ])
        .await?;

    let output = result.stdout.lines().next().expect("an output line");
    assert!(buck.cwd().join(output).exists());

    let res = buck
        .bxl(&[
            "//executor_fallback_tests/dynamic.bxl:test_dynamic_output",
            "-c",
            &format!("test.cache_buster={}", random_string()),
            "--remote-only",
        ])
        .await
        .expect_failure();
    res.assert_stderr_matches("Incompatible executor preferences");

    let record = res.invocation_record()?;
    let errors = record["errors"].as_array().expect("`errors` is a list");

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["category"], "USER");
    Ok(())
}

#[buck_test]
async fn test_bxl_execution_platforms(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&[
            "//executor_fallback_tests/test.bxl:test_exec_platforms",
            "-c",
            &format!("test.cache_buster={}", random_string()),
            "--",
            "--exec_deps",
            "//executor_fallback_tests:remote_only",
        ])
        .await?;

    let output = result.stdout.lines().next().expect("an output line");
    assert!(buck.cwd().join(output).exists());

    buck.bxl(&[
        "//executor_fallback_tests/test.bxl:test_exec_platforms",
        "-c",
        &format!("test.cache_buster={}", random_string()),
        "--",
        "--exec_deps",
        "//executor_fallback_tests:local_only",
    ])
    .await
    .expect_failure();

    let result = buck
        .bxl(&[
            "//executor_fallback_tests/test.bxl:test_exec_platforms",
            "-c",
            &format!("test.cache_buster={}", random_string()),
            "--",
            "--toolchains",
            "//executor_fallback_tests:remote_only_toolchain",
        ])
        .await?;

    let output = result.stdout.lines().next().expect("an output line");
    assert!(buck.cwd().join(output).exists());

    buck.bxl(&[
        "//executor_fallback_tests/test.bxl:test_exec_platforms",
        "-c",
        &format!("test.cache_buster={}", random_string()),
        "--",
        "--toolchains",
        "//executor_fallback_tests:local_only_toolchain",
    ])
    .await
    .expect_failure();

    let result = buck
        .bxl(&[
            "//executor_fallback_tests/test.bxl:test_exec_compatible_with",
            "-c",
            &format!("test.cache_buster={}", random_string()),
        ])
        .await?;

    let output = result.stdout.lines().next().expect("an output line");
    assert!(buck.cwd().join(output).exists());

    buck.bxl(&[
        "//executor_fallback_tests/test.bxl:test_exec_compatible_with",
        "-c",
        &format!("test.cache_buster={}", random_string()),
        "--remote-only",
    ])
    .await
    .expect_failure();
    Ok(())
}
