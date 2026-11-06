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

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_catch_resolve_lazy_dict(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/utils:test_lazy_utils.bxl:test_catch_resolve_lazy_dict"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_batch_apply_lazy(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/utils:test_lazy_utils.bxl:test_batch_apply_lazy"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_batch_apply_lazy_catch_each(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/utils:test_lazy_utils.bxl:test_batch_apply_lazy_catch_each"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_batch_apply_lazy_catch_all(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/utils:test_lazy_utils.bxl:test_batch_apply_lazy_catch_all"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_partition_results(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/utils:test_lazy_utils.bxl:test_partition_results"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_partition_results_dict(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/utils:test_lazy_utils.bxl:test_partition_results_dict"])
        .await?;
    Ok(())
}

// dummy test to avoid test listing failure on windows
#[buck_test(inplace = true)]
async fn test_dummy(_buck: &Buck) {}
