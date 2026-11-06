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
async fn test_lazy_unconfigured_target(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_unconfigured_target_node.bxl:lazy_unconfigured_target_node_resolve"])
        .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_unconfigured_target_error(buck: &Buck) {
    buck.bxl(&[
        "//bxl/lazy_unconfigured_target_node.bxl:lazy_unconfigured_target_node_resolve_error",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("error: Unknown target `not_exist` from package `root//bin`.");
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_unconfigured_target_catch_error(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[
        "//bxl/lazy_unconfigured_target_node.bxl:lazy_unconfigured_target_node_resolve_catch_error",
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple")]
async fn test_lazy_unconfigured_target_node_pattern(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/lazy_unconfigured_target_node.bxl:lazy_unconfigured_target_node_pattern"])
        .await?;
    Ok(())
}
