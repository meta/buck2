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
async fn test_visibility_from_package_simple(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&["root//simple:", "--output-attribute=visibility|within_view"])
        .await?;
    golden(&result.stdout, "simple/golden.uquery.json");
    Ok(())
}

#[buck_test]
async fn test_visibility_from_package_inherit(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[
            "root//inherit/...",
            "--output-attribute=visibility|within_view",
        ])
        .await?;
    golden(&result.stdout, "inherit/golden.uquery.json");
    Ok(())
}

#[buck_test]
async fn test_visibility_from_package_override(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[
            "root//override/...",
            "--output-attribute=visibility|within_view",
        ])
        .await?;
    golden(&result.stdout, "override/golden.uquery.json");
    Ok(())
}

#[buck_test]
async fn test_visibility_from_package_public(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .uquery(&[
            "root//public/...",
            "--output-attribute=visibility|within_view",
        ])
        .await?;
    golden(&result.stdout, "public/golden.uquery.json");
    Ok(())
}
