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
use buck2_e2e::utils::read_what_ran;

/// Runs `target` twice, with a different seed each time: the first run makes the incremental
/// action count from 0, the second from where the first left it.
async fn run_twice(buck: &Buck, target: &str) -> anyhow::Result<()> {
    let result = buck
        .run(&[target, "-c", &format!("test.seed={}", random_string())])
        .await?;
    assert_eq!(result.stdout, "0");
    let result = buck
        .run(&[target, "-c", &format!("test.seed={}", random_string())])
        .await?;
    assert_eq!(result.stdout, "1");
    Ok(())
}

#[buck_test]
async fn test_incremental_file_materialized(buck: &Buck) -> anyhow::Result<()> {
    run_twice(buck, "root//:plate").await
}

#[buck_test]
async fn test_incremental_dir_materialized(buck: &Buck) -> anyhow::Result<()> {
    run_twice(buck, "root//:mate").await
}

#[buck_test]
async fn test_incremental_file_not_materialized(buck: &Buck) -> anyhow::Result<()> {
    run_twice(buck, "root//:flute").await
}

#[buck_test]
async fn test_incremental_dir_not_materialized(buck: &Buck) -> anyhow::Result<()> {
    run_twice(buck, "root//:suite").await
}

#[buck_test]
async fn test_remote_cache_is_used(buck: &Buck) -> anyhow::Result<()> {
    let seed = random_string();
    let result = buck
        .run(&["root//:plate", "-c", &format!("test.seed={seed}")])
        .await?;
    assert_eq!(result.stdout, "0");
    let result = buck
        .run(&[
            "root//:plate",
            "-c",
            &format!("test.seed={}", random_string()),
        ])
        .await?;
    assert_eq!(result.stdout, "1");

    // For the next build with already used seed we expect the action to be taken from the cache
    let result = buck
        .run(&["root//:plate", "-c", &format!("test.seed={seed}")])
        .await?;
    assert_eq!(result.stdout, "0");

    let out = read_what_ran(buck, &[]).await?;
    assert_eq!(out.len(), 1, "out should have 1 line: `{out:?}`");
    assert_eq!(out[0]["reproducer"]["executor"], "Cache");
    Ok(())
}
