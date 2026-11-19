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
async fn test_bxl_caching(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//caching.bxl:print_caching"]).await?;

    assert!(result.stderr.contains("ran me"));
    assert!(result.stdout.contains("result print"));

    let result = buck.bxl(&["//caching.bxl:print_caching"]).await?;

    assert!(!result.stderr.contains("ran me"));
    assert!(result.stdout.contains("result print"));
    Ok(())
}

#[buck_test]
async fn test_bxl_caching_with_target_platforms_specified(buck: &Buck) -> anyhow::Result<()> {
    // run with platform1, result should be cached afterwards
    let result = buck
        .bxl(&[
            "//caching.bxl:caching_with_target_platforms",
            "--target-platforms",
            "root//:platform1",
        ])
        .await?;

    assert!(result.stderr.contains("ran me"));
    assert!(result.stdout.contains("root//:platform1"));

    // run with platform2, DICE should be invalidated and updated results should be
    // cached afterwards
    let result = buck
        .bxl(&[
            "//caching.bxl:caching_with_target_platforms",
            "--target-platforms",
            "root//:platform2",
        ])
        .await?;

    assert!(result.stderr.contains("ran me"));
    assert!(result.stdout.contains("root//:platform2"));

    // run with platform1 again, we should already have cached results
    let result = buck
        .bxl(&[
            "//caching.bxl:caching_with_target_platforms",
            "--target-platforms",
            "root//:platform1",
        ])
        .await?;

    assert!(!result.stderr.contains("ran me"));
    assert!(result.stdout.contains("root//:platform1"));
    Ok(())
}

#[buck_test]
async fn test_bxl_error_caching(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//caching.bxl:print_error_caching"]).await?;
    assert!(result.stderr.contains("ran me"));
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(result.stderr.contains("root//:incompatible"));

    // output stream that writes to stderr should be cached, but regular stdlib print
    // statements (which also write to stderr) will not be cached.
    let result = buck.bxl(&["//caching.bxl:print_error_caching"]).await?;
    assert!(!result.stderr.contains("ran me"));
    assert!(result.stderr.contains("Skipped 1 incompatible targets"));
    assert!(result.stderr.contains("root//:incompatible"));
    Ok(())
}

#[buck_test]
async fn test_bxl_print_with_no_buckd(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//caching.bxl:print_caching", "--no-buckd"])
        .await?;

    assert!(result.stderr.contains("ran me"));
    assert!(result.stdout.contains("result print"));
    Ok(())
}
