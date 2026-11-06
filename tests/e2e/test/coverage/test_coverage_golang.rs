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

fn contains(paths: &[String], path: &str) -> bool {
    paths.iter().any(|p| p == path)
}

#[buck_test(inplace = true)]
async fn test_go_test_dbgo_cov(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//test_frameworks/gotest/playground:simple_add_test",
        "--",
        &format!("--coverage-output={}", coverage_file.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&coverage_file)?;

    assert!(
        contains(&paths, "fbcode/test_frameworks/gotest/playground/simple.go"),
        "{paths:?}"
    );
    assert!(
        contains(
            &paths,
            "fbcode/test_frameworks/gotest/playground/simple_add_test.go"
        ),
        "{paths:?}"
    );
    Ok(())
}

/// Test that filtered coverage produces coverage data when folder_path_filter matches.
#[buck_test(inplace = true)]
async fn test_go_test_filtered_coverage_by_folder(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//test_frameworks/gotest/playground:simple_add_test",
        &["fbcode/test_frameworks/gotest/playground"],
        &[],
    )
    .await?;

    assert!(
        contains(&paths, "fbcode/test_frameworks/gotest/playground/simple.go"),
        "{paths:?}"
    );
    assert!(
        contains(
            &paths,
            "fbcode/test_frameworks/gotest/playground/simple_add_test.go"
        ),
        "{paths:?}"
    );
    Ok(())
}

/// Test that filtered coverage produces coverage data when file_path_filter matches.
#[buck_test(inplace = true)]
async fn test_go_test_filtered_coverage_by_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//test_frameworks/gotest/playground:simple_add_test",
        &[],
        &["fbcode/test_frameworks/gotest/playground/simple.go"],
    )
    .await?;

    assert!(
        contains(&paths, "fbcode/test_frameworks/gotest/playground/simple.go"),
        "{paths:?}"
    );
    Ok(())
}

/// Test selective coverage with file_path_filter on a different test target.
#[buck_test(inplace = true)]
async fn test_go_test_selective_coverage_by_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/go:math_test",
        &[],
        &["fbcode/testing_frameworks/code_coverage/go/add.go"],
    )
    .await?;

    assert!(
        contains(&paths, "fbcode/testing_frameworks/code_coverage/go/add.go"),
        "{paths:?}"
    );
    Ok(())
}

/// Test that filtered coverage produces no coverage data when folder_path_filter doesn't match.
#[buck_test(inplace = true)]
async fn test_go_test_filtered_coverage_not_matching(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//test_frameworks/gotest/playground:simple_add_test",
        &["some/other/path"],
        &[],
    )
    .await?;

    assert!(
        paths.is_empty(),
        "Expected no coverage data but got: {paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_go_test_dbgo_cov_on_remote_execution(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//test_frameworks/gotest/playground:simple_add_test_re",
        "--",
        &format!("--coverage-output={}", coverage_file.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&coverage_file)?;

    assert!(
        contains(&paths, "fbcode/test_frameworks/gotest/playground/simple.go"),
        "{paths:?}"
    );
    assert!(
        contains(
            &paths,
            "fbcode/test_frameworks/gotest/playground/simple_add_test.go"
        ),
        "{paths:?}"
    );
    Ok(())
}
