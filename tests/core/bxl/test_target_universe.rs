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
async fn test_bxl_target_universe_keep_going_no_errors(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//target_universe.bxl:target_universe_keep_going_no_errors"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_universe_target_set(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//target_universe.bxl:target_universe_universe_target_set"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_with_errors(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//keep_going.bxl:target_universe_keep_going_with_errors"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_list_input(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//keep_going.bxl:target_universe_keep_going_list_input"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_target_set_input(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//keep_going.bxl:target_universe_keep_going_target_set_input"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_mixed_list(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//keep_going.bxl:target_universe_keep_going_mixed_list"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_all_fail(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//keep_going.bxl:target_universe_keep_going_all_fail"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_incompatible_target_set(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//keep_going.bxl:target_universe_keep_going_incompatible_target_set"])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible_target")
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_incompatible_string_pattern(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//keep_going.bxl:target_universe_keep_going_incompatible_string_pattern"])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible_target")
    );
    Ok(())
}

#[buck_test]
async fn test_bxl_target_universe_keep_going_incompatible_list(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//keep_going.bxl:target_universe_keep_going_incompatible_list"])
        .await?;
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(
        result
            .stderr
            .contains("root//incompatible_targets:incompatible_target")
    );
    Ok(())
}
