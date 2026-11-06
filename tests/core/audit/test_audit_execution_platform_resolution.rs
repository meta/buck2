/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::golden::golden_replace_cfg_hash;
use buck2_e2e::prelude::*;

#[buck_test]
async fn test_audit_execition_platform_resolution(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .audit(&["execution-platform-resolution", "//:target"])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "out.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_audit_execution_platform_resolution_no_compatible_platform(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .audit(&["execution-platform-resolution", "//:no_compatible_platform"])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "no_compatible_platform.txt.golden");
    Ok(())
}

#[buck_test]
async fn test_audit_execution_platform_resolution_error_fallback(
    buck: &Buck,
) -> anyhow::Result<()> {
    // With `fallback = "error"` the target fails to configure outright, so there is no
    // retained resolution to read the skip reasons from; the audit command reconstructs
    // them by re-walking the candidates.
    let result = buck
        .audit(&[
            "execution-platform-resolution",
            "//:no_compatible_platform",
            "--setting",
            "execution_platforms.target=root//execution_platforms:execution_platforms_error_fallback",
        ])
        .await?;
    golden_replace_cfg_hash(&result.stdout, "error_fallback.txt.golden");
    Ok(())
}
