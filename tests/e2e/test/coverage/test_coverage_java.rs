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

const JAVA_TEST_TARGET: &str =
    "fbsource//xplat/test_frameworks/coverage/java/playground:SimpleTest";

const EXTRA_BUCK_ARGS: &[&str] = &[
    "--config",
    "junit_selective_coverage_rollout.is_enabled=true",
    // currently selective coverage is enabled via this flag
];

/// A coverage run of `target`'s tests where only the files under `folders` and the files in
/// `files` are instrumented. `mode` and `extra_buck_args` go to `buck2 test` too.
struct CoverageRun<'a> {
    target: &'a str,
    folders: &'a [&'a str],
    files: &'a [&'a str],
    mode: Option<&'a str>,
    extra_buck_args: &'a [&'a str],
}

/// The files the coverage of `run` covers, without duplicates.
async fn collect_coverage_for(
    buck: &Buck,
    tmp_path: &Path,
    run: CoverageRun<'_>,
) -> anyhow::Result<Vec<String>> {
    let CoverageRun {
        target,
        folders,
        files,
        mode,
        extra_buck_args,
    } = run;
    let coverage_file = tmp_path.join("coverage.txt");
    let folder_filter = format!("code_coverage.folder_path_filter={}", folders.join(":"));
    let file_filter = format!("code_coverage.file_path_filter={}", files.join(":"));
    let coverage_output = format!("--coverage-output={}", coverage_file.display());
    let mut args: Vec<&str> = mode.into_iter().collect();
    args.extend([
        "--config",
        "code_coverage.enable=filtered",
        "--config",
        &folder_filter,
        "--config",
        &file_filter,
    ]);
    args.extend(extra_buck_args);
    args.extend([target, "--", "--collect-coverage", &coverage_output]);
    buck.test(&args).await?;
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

async fn java_coverage_file_filter(
    buck: &Buck,
    tmp_path: &Path,
    mode: Option<&str>,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "xplat/test_frameworks/coverage/java/playground/java_test/src/test/com/facebook/playground/SimpleTest.java";
    let result = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: JAVA_TEST_TARGET,
            folders: &[],
            files: &[file_to_collect_coverage],
            mode,
            extra_buck_args: EXTRA_BUCK_ARGS,
        },
    )
    .await?;

    assert_eq!(
        result,
        [file_to_collect_coverage],
        "Only {file_to_collect_coverage} should have coverage, instead got coverage for {result:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_java_coverage_file_filter_None(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    java_coverage_file_filter(buck, tmp_path, None).await
}

#[buck_test(inplace = true)]
async fn test_java_coverage_file_filter_fbcode_mode_dev(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    java_coverage_file_filter(buck, tmp_path, Some("@fbcode//mode/dev")).await
}

#[buck_test(inplace = true)]
async fn test_java_coverage_file_filter_fbcode_mode_opt(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    java_coverage_file_filter(buck, tmp_path, Some("@fbcode//mode/opt")).await
}

async fn java_coverage_folder_filter(
    buck: &Buck,
    tmp_path: &Path,
    mode: Option<&str>,
) -> anyhow::Result<()> {
    let folder_to_collect_coverage = "xplat/test_frameworks/coverage/java/playground/java_test/src/test/com/facebook/playground/nested";
    let result = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: JAVA_TEST_TARGET,
            folders: &[folder_to_collect_coverage],
            files: &[],
            mode,
            extra_buck_args: EXTRA_BUCK_ARGS,
        },
    )
    .await?;

    let expected_files = [
        "xplat/test_frameworks/coverage/java/playground/java_test/src/test/com/facebook/playground/nested/Adder.java",
    ];
    assert_eq!(
        result, expected_files,
        "Only {expected_files:?} should have coverage, instead got coverage for {result:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
#[expect(non_snake_case, reason = "the Python harness's name for the case")]
async fn test_java_coverage_folder_filter_None(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    java_coverage_folder_filter(buck, tmp_path, None).await
}

#[buck_test(inplace = true)]
async fn test_java_coverage_folder_filter_fbcode_mode_dev(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    java_coverage_folder_filter(buck, tmp_path, Some("@fbcode//mode/dev")).await
}

#[buck_test(inplace = true)]
async fn test_java_coverage_folder_filter_fbcode_mode_opt(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    java_coverage_folder_filter(buck, tmp_path, Some("@fbcode//mode/opt")).await
}

#[buck_test(inplace = true)]
async fn test_junit_test_selective_coverage_doesnt_produce_coverage(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/junit/com/facebook/testing_frameworks:test",
            folders: &[],
            files: &["testing_frameworks/code_coverage/junit/com/facebook/testing_frameworks/AddTest.java"],
            mode: None,
            extra_buck_args: &[],
        },
    )
    .await?;

    assert!(paths.is_empty(), "{paths:?}");
    Ok(())
}
