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
async fn test_build_transition_without_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build_without_report(&[
            "root//:buck",
            "--target-platforms=root//:p",
            "--show-output",
        ])
        .await?;

    let lines: Vec<&str> = result.stdout.lines().collect();
    // Just a single target is built and output
    assert_eq!(1, lines.len());
    assert!(lines[0].contains("root//:buck buck-out"));
    Ok(())
}

#[buck_test]
async fn test_build_transition_with_target_universe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build_without_report(&[
            "root//:buck",
            "--target-platforms=root//:p",
            "--target-universe",
            "root//:buck",
            "--show-output",
        ])
        .await?;

    let lines: Vec<&str> = result.stdout.lines().collect();
    // Just a single target is built and output
    assert_eq!(1, lines.len());
    assert!(lines[0].contains("root//:buck buck-out"));
    Ok(())
}
