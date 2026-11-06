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
async fn test_apple_coverage(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let covfile = tmp_path.join("coverage.txt");
    buck.test(&[
        "-c",
        "xplat.available_platforms=APPLE,CXX",
        "-c",
        "code_coverage.enable=all",
        "fbsource//fbobjc/Samples/TestInfra/TpxUnitTests:TpxUnitTests",
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", covfile.display()),
        "--return-zero-on-skips",
    ])
    .await?;
    let paths = read_coverage_paths(&covfile)?;
    assert!(
        paths
            .iter()
            .any(|p| p == "fbobjc/Samples/TestInfra/TpxUnitTests/TpxUnitTests/TpxUnitTests.m"),
        "{paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_apple_coverage_xplat(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let covfile = tmp_path.join("coverage.txt");
    buck.test(&[
        "-c",
        "xplat.available_platforms=APPLE,CXX",
        "-c",
        "code_coverage.enable=all",
        // By default, xplat targets currently use xbat to compile to apple.
        // With xbat, however, we'll get divergence between the LLVM tooling
        // during pika upgrades, as pika will be newer than what's provided
        // by xbat.
        "@fbsource//fbobjc/mode/buck2/toolchains/pika-fat",
        "fbsource//xplat/testinfra/playground/cpp:example_testApple",
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", covfile.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&covfile)?;
    assert!(
        paths
            .iter()
            .any(|p| p == "xplat/testinfra/playground/cpp/ExampleTest.cpp"),
        "{paths:?}"
    );
    Ok(())
}
