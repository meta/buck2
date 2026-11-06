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

#[buck_test]
async fn test_run_executable(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.run(&["root//:print_animal_hello"]).await?;
    assert_eq!(result.stdout.trim(), "hello dog");

    let result = buck
        .run(&[
            "root//:print_animal_hello",
            "--target-universe",
            "root//:cat_universe",
        ])
        .await?;
    assert_eq!(result.stdout.trim(), "hello cat");
    Ok(())
}

#[buck_test]
async fn test_run_with_transition_without_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .run(&["root//:buck", "--target-platforms=root//:p_cat"])
        .await?;

    // The transition (deliberately) loses the configuration so that we get the
    // DEFAULT 'hello buck' from the select in the target definition.
    assert_eq!(result.stdout.trim(), "hello buck");
    Ok(())
}

#[buck_test]
async fn test_run_with_transition_with_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .run(&[
            "root//:buck",
            "--target-platforms=root//:p_cat",
            "--target-universe",
            "root//:buck",
        ])
        .await?;

    // The transition (deliberately) loses the configuration so that we get the
    // DEFAULT 'hello buck' from the select in the target definition.
    assert_eq!(result.stdout.trim(), "hello buck");
    Ok(())
}

#[buck_test]
async fn test_run_target_not_in_universe(buck: &Buck) {
    buck.run(&[
        "root//:print_animal_hello",
        "--target-universe",
        "root//:print_animal_goodbye",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(
        "Target `root//:print_animal_hello` is not found in the specified target universe",
    );
}
