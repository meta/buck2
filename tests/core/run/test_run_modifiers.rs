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

/// The `os cpu` that `root//:run` prints.
fn os_and_cpu(stdout: &str) -> (&str, &str) {
    match stdout.split_whitespace().collect::<Vec<_>>()[..] {
        [os, cpu] => (os, cpu),
        _ => panic!("Expected `os cpu`, got `{stdout}`"),
    }
}

#[buck_test]
async fn test_run_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:run?root//:macos"]).await?;

    let (os, cpu) = os_and_cpu(&result.stdout);

    assert_eq!(os, "macos");
    assert_eq!(cpu, "DEFAULT");
    Ok(())
}

#[buck_test]
async fn test_run_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:run?root//:macos+root//:arm"]).await?;

    let (os, cpu) = os_and_cpu(&result.stdout);

    assert_eq!(os, "macos");
    assert_eq!(cpu, "arm");
    Ok(())
}

#[buck_test]
async fn test_run_order_of_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:run?root//:macos+root//:linux"]).await?;

    let (os, cpu) = os_and_cpu(&result.stdout);

    assert_eq!(os, "linux");
    assert_eq!(cpu, "DEFAULT");
    Ok(())
}

#[buck_test]
async fn test_run_target_universe_single_modifier(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .run(&["root//:run", "--target-universe", "root//:run?root//:macos"])
        .await?;

    let (os, cpu) = os_and_cpu(&result.stdout);

    assert_eq!(os, "macos");
    assert_eq!(cpu, "DEFAULT");
    Ok(())
}

#[buck_test]
async fn test_run_target_universe_multiple_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:run",
            "--target-universe",
            "root//:run?root//:macos+root//:arm",
        ])
        .await?;

    let (os, cpu) = os_and_cpu(&result.stdout);

    assert_eq!(os, "macos");
    assert_eq!(cpu, "arm");
    Ok(())
}

#[buck_test]
async fn test_run_fails_with_global_modifiers(buck: &Buck) {
    buck.run(&["--modifier", "root//:macos", "root//:run?root//:linux"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
        );

    buck.run(&[
        "--modifier",
        "root//:macos",
        "root//:run",
        "--target-universe",
        "root//:run?root//:linux",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag",
    );
}

#[buck_test]
async fn test_run_fails_with_pattern_modifier_and_target_universe_modifier(buck: &Buck) {
    buck.run(&[
        "root//:run?root//:macos",
        "--target-universe",
        "root//:run?root//:arm",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        r"Cannot use \?modifier syntax in target pattern expression with --target-universe flag",
    );
}
