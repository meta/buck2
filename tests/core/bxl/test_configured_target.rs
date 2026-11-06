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

#[buck_test(data_dir = "")]
async fn test_unwrap_forward(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/configured_target.bxl:unwrap_forward"])
        .await?;
    Ok(())
}

#[buck_test(data_dir = "")]
async fn test_configured_targets_with_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .bxl(&["//bxl/configured_target.bxl:configured_targets_with_modifiers"])
        .await?;
    let configurations: Vec<&str> = result.stdout.lines().map(str::trim).collect();
    let linux_cfg = buck.audit_configurations(&[configurations[0]]).await?;
    assert!(linux_cfg.stdout.contains("root//:linux"));
    let macos_cfg = buck.audit_configurations(&[configurations[1]]).await?;
    assert!(macos_cfg.stdout.contains("root//:macos"));
    Ok(())
}

#[buck_test(data_dir = "")]
async fn test_strip_cfg(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/configured_target.bxl:strip_cfg"]).await?;
    Ok(())
}
