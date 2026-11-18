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
async fn test_get_package_path(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:get_package_path"]).await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_value(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_value"]).await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_value_from_string(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_value_from_string"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_read_override_package_value(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_override_package_value"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_value_not_found(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_value_not_found"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_visibility(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_visibility"]).await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_within_view(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_within_view"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_visibility_cap(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_visibility_cap"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_within_view_cap(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_within_view_cap"])
        .await?;
    Ok(())
}

#[buck_test]
async fn test_read_package_visibility_intersection(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//package.bxl:read_package_visibility_intersection"])
        .await?;
    Ok(())
}
