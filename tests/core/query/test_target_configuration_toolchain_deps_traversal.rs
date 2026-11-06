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

// Test `target_deps()` function does not include toolchain deps.
#[buck_test]
async fn test_cquery_target_deps(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.cquery(&["deps(tests/..., 1, target_deps())"]).await?;
    // TODO(nga): this test does not test that any target deps are actually returned.
    golden_replace_cfg_hash(&result.stdout, "cquery_target_deps.golden");
    Ok(())
}

// Test `target_deps()` function does not include toolchain deps.
#[buck_test]
async fn test_uquery_target_deps(buck: &Buck) -> anyhow::Result<()> {
    // TODO(nga): output includes `platform_windows` target, which is probably not meant to be there.
    let result = buck.uquery(&["deps(tests/..., 1, target_deps())"]).await?;
    golden(&result.stdout, "uquery_target_deps.golden");
    Ok(())
}

// Test `configuration_deps()` function does include configuration deps.
#[buck_test]
async fn test_cquery_configuration_deps(buck: &Buck) -> anyhow::Result<()> {
    let q = "deps(tests/..., 1, configuration_deps())";
    let result = buck.cquery(&[q]).await?;
    // Note test output includes `root//tests:python_only`, which is not a configuration deps.
    // This is now `deps()` with traversal function works: it includes roots.
    golden_replace_cfg_hash(&result.stdout, "cquery_configuration_deps.golden");
    Ok(())
}

// Test `configuration_deps()` function does include configuration deps.
#[buck_test]
async fn test_uquery_configuration_deps(buck: &Buck) -> anyhow::Result<()> {
    let q = "deps(tests/..., 1, configuration_deps())";
    let result = buck.uquery(&[q]).await?;
    // TODO(nga): this does not return any configuration deps.
    golden(&result.stdout, "uquery_configuration_deps.golden");
    Ok(())
}

#[buck_test]
async fn test_cquery_toolchain_deps(buck: &Buck) -> anyhow::Result<()> {
    let q = "deps(tests:python_and_asic, 1, toolchain_deps())";
    let out = buck.cquery(&[q]).await?;
    golden_replace_cfg_hash(&out.stdout, "cquery_toolchain_deps.golden");
    Ok(())
}

#[buck_test]
async fn test_uquery_toolchain_deps(buck: &Buck) -> anyhow::Result<()> {
    let q = "deps(tests:python_and_asic, 1, toolchain_deps())";
    let out = buck.uquery(&[q]).await?;
    golden(&out.stdout, "uquery_toolchain_deps.golden");
    Ok(())
}
