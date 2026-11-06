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

/// The files the coverage of `target`'s tests covers, without duplicates, when only the files
/// under `folder_filter` and the files in `file_filter` are instrumented.
async fn collect_coverage_for(
    buck: &Buck,
    tmp_path: &Path,
    target: &str,
    folder_filter: &[&str],
    file_filter: &[&str],
) -> anyhow::Result<Vec<String>> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "--config",
        "code_coverage.enable=filtered",
        "--config",
        &format!(
            "code_coverage.folder_path_filter={}",
            folder_filter.join(":")
        ),
        "--config",
        &format!("code_coverage.file_path_filter={}", file_filter.join(":")),
        target,
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", coverage_file.display()),
    ])
    .await?;
    let paths: BTreeSet<String> = read_coverage_paths(&coverage_file)?.into_iter().collect();
    Ok(paths.into_iter().collect())
}

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
async fn test_junit_test_selective_coverage_doesnt_produce_coverage(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/junit/com/facebook/testing_frameworks:test",
        &[],
        &["testing_frameworks/code_coverage/junit/com/facebook/testing_frameworks/AddTest.java"],
    )
    .await?;

    assert!(paths.is_empty(), "{paths:?}");
    Ok(())
}
