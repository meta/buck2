/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;

/// The `filepath` of every entry of the coverage file tpx wrote.
fn read_coverage_paths(coverage_file: &Path) -> anyhow::Result<Vec<String>> {
    std::fs::read_to_string(coverage_file)?
        .lines()
        .map(|line| {
            let entry: serde_json::Value = serde_json::from_str(line)?;
            entry
                .get("filepath")
                .and_then(|p| p.as_str())
                .map(str::to_owned)
                .with_context(|| format!("No `filepath` in the coverage entry `{line}`"))
        })
        .collect()
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_with_network_sandboxing(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        "-c",
        // We need to build the llvm coverage wrapper from source because otherwise this is served
        // via msdk which will fail to download the wrapper when wrapped in an `unshare` call.
        "coverage.llvm_build_from_source=true",
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", coverage_file.display()),
        "--experimental-force-linux-network-sandboxing",
    ])
    .await?;
    let paths = read_coverage_paths(&coverage_file)?;

    assert!(
        paths
            .iter()
            .any(|p| p == "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp"),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p == "fbcode/common/gtest/LightMain.cpp"),
        "{paths:?}"
    );
    Ok(())
}
