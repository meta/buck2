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

/// Test unconfigured_targets_keep_going with a specific successful target.
#[buck_test]
async fn test_specific_target_success(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:unconfigured_targets_keep_going.bxl:test_specific_target_success"])
        .await?;
    Ok(())
}

/// Test unconfigured_targets_keep_going with a recursive pattern that includes only successful packages.
#[buck_test]
async fn test_recursive_pattern_success(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:unconfigured_targets_keep_going.bxl:test_recursive_pattern_success"])
        .await?;
    Ok(())
}

/// Test unconfigured_targets_keep_going with a recursive pattern that includes both successful and failing packages.
#[buck_test]
async fn test_recursive_pattern_mixed(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:unconfigured_targets_keep_going.bxl:test_recursive_pattern_mixed"])
        .await?;
    Ok(())
}

/// Test unconfigured_targets_keep_going with a pattern that only matches a failing package.
#[buck_test]
async fn test_failing_package_only(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:unconfigured_targets_keep_going.bxl:test_failing_package_only"])
        .await?;
    Ok(())
}

/// Test unconfigured_targets_keep_going with a specific target in a failing package.
#[buck_test]
async fn test_specific_target_in_failing_package(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//:unconfigured_targets_keep_going.bxl:test_specific_target_in_failing_package"])
        .await?;
    Ok(())
}
