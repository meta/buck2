/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use anyhow::Context;
use buck2_e2e::prelude::*;

// Test `cfg_constructors` end to end. This is useful for testing the core + starlark
// implementation so that we know we don't break anything in the repo. For testing
// specific cfg constructor logic, use bxl_test to unit test the cfg constructor instead

/// The `buck.target_configuration` of the one target `cquery -A` printed.
fn target_configuration(stdout: &str) -> anyhow::Result<String> {
    let result: serde_json::Map<String, serde_json::Value> = serde_json::from_str(stdout)?;
    assert_eq!(result.len(), 1);
    let (_test_target, test_target_attrs) = result.iter().next().context("No target")?;
    Ok(test_target_attrs
        .get("buck.target_configuration")
        .and_then(serde_json::Value::as_str)
        .context("No `buck.target_configuration`")?
        .to_owned())
}

#[buck_test(inplace = true)]
async fn test_cfg_constructor_without_modifiers_returns_same_configuration(
    buck: &Buck,
) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/test_cfg_constructor_data:no_modifiers",
            "-A",
        ])
        .await?;
    assert!(target_configuration(&result.stdout)?.starts_with("ovr_config//platform:base"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cfg_constructor_with_target_modifiers(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .cquery(&[
            "fbcode//buck2/tests/e2e/configurations/cfg_constructor/test_clear_package_modifiers_data/test_cfg_constructor_data:has_target_modifier",
            "-A",
        ])
        .await?;
    assert!(target_configuration(&result.stdout)?.starts_with("cfg:linux"));
    Ok(())
}
