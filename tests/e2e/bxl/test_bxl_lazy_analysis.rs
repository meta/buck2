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

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_analysis_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:lazy_analysis_resolve"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_analysis_resolve_error(buck: &Buck) {
    buck.bxl(&["//bxl/lazy_analysis.bxl:lazy_analysis_resolve_error"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "requested sub target named `missing_subtarget` .* is not available",
        );
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_analysis_try_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:lazy_analysis_try_resolve"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_analysis_try_resolve_error(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:lazy_analysis_try_resolve_error"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_batch_lazy_analysis_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:batch_lazy_analysis_resolve"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_batch_lazy_analysis_resolve_error(buck: &Buck) {
    buck.bxl(&["//bxl/lazy_analysis.bxl:batch_lazy_analysis_resolve_error"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "requested sub target named `missing_subtarget` .* is not available",
        );
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_batch_lazy_analysis_try_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:batch_lazy_analysis_try_resolve"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_join_lazy_analysis_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:join_lazy_analysis_resolve"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_join_lazy_analysis_resolve_error(buck: &Buck) {
    buck.bxl(&["//bxl/lazy_analysis.bxl:join_lazy_analysis_resolve_error"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "requested sub target named `missing_subtarget` .* is not available",
        );
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_join_lazy_analysis_try_resolve(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_analysis.bxl:join_lazy_analysis_try_resolve"])
        .await?;
    Ok(())
}
