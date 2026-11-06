/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeSet;
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
async fn test_python_coverage(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let covfile = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//buck2/tests/targets/rules/python/coverage:test",
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", covfile.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&covfile)?;
    assert!(
        paths
            .iter()
            .any(|p| p == "fbcode/buck2/tests/targets/rules/python/coverage/lib.py"),
        "{paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_folder(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let folder_to_collect = "buck2/tests/targets/rules/python/coverage";
    let covfile = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//buck2/tests/targets/rules/python/coverage:test",
        "-c",
        &format!("fbcode.cxx_coverage_only={folder_to_collect}"),
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", covfile.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&covfile)?;
    let lib = format!("fbcode/{folder_to_collect}/lib.py");
    let test = format!("fbcode/{folder_to_collect}/test.py");
    assert_eq!(
        paths.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([lib.as_str(), test.as_str()]),
        "Only folder fbcode/{folder_to_collect} should have coverage, instead got coverage for {paths:?}"
    );
    Ok(())
}
