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

const PYTHON_TEST_EXAMPLE_TARGET: &str = "fbcode//buck2/tests/targets/rules/python/coverage:test";

/// A coverage run of `target`'s tests where only the files under `folders` and the files in
/// `files` are instrumented. `mode` goes to `buck2 test` too.
struct CoverageRun<'a> {
    target: &'a str,
    folders: &'a [&'a str],
    files: &'a [&'a str],
    mode: Option<&'a str>,
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
        target,
        "--",
        "--collect-coverage",
        &coverage_output,
    ]);
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

/// Collects the coverage of `target`'s tests with only `file_to_collect_coverage` instrumented,
/// and checks that it is the only file covered.
async fn assert_only_file_covered(
    buck: &Buck,
    tmp_path: &Path,
    target: &str,
    file_to_collect_coverage: &str,
    mode: Option<&str>,
) -> anyhow::Result<()> {
    let result = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target,
            folders: &[],
            files: &[file_to_collect_coverage],
            mode,
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
async fn test_python_coverage(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let covfile = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        PYTHON_TEST_EXAMPLE_TARGET,
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
    let folder_to_collect = "fbcode/buck2/tests/targets/rules/python/coverage";
    let covfile = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        PYTHON_TEST_EXAMPLE_TARGET,
        "-c",
        &format!("code_coverage.folder_path_filter={folder_to_collect}"),
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", covfile.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&covfile)?;
    let lib = format!("{folder_to_collect}/lib.py");
    let test = format!("{folder_to_collect}/test.py");
    assert_eq!(
        paths.iter().map(String::as_str).collect::<BTreeSet<_>>(),
        BTreeSet::from([lib.as_str(), test.as_str()]),
        "Only folder {folder_to_collect} should have coverage, instead got coverage for {paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        PYTHON_TEST_EXAMPLE_TARGET,
        "fbcode/buck2/tests/targets/rules/python/coverage/lib.py",
        None,
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_file_with_base_module_remap(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/python/coverage:test_using_base_module_remapped_lib",
        "fbcode/buck2/tests/targets/rules/python/coverage/lib.py",
        None,
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_file_with_opt_mode(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        PYTHON_TEST_EXAMPLE_TARGET,
        "fbcode/buck2/tests/targets/rules/python/coverage/lib.py",
        Some("@fbcode//mode/opt"),
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_standalone_xar_coverage_filtering_by_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbcode//testinfra/playground/python/simple:simple_standalone_xar_test",
        "fbcode/testinfra/playground/python/simple/simple_test.py",
        Some("@fbcode//mode/opt"),
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_standalone_zip_coverage_filtering_by_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbcode//testinfra/playground/python/simple:simple_standalone_test",
        "fbcode/testinfra/playground/python/simple/simple_test.py",
        Some("@fbcode//mode/opt"),
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_file_on_cinder_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/python/coverage:simple_cinder_unittest",
        "fbcode/buck2/tests/targets/rules/python/coverage/test.py",
        None,
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_source_file_on_cpp_dep(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/python/coverage:test_with_cpp_dep",
        "fbcode/buck2/tests/targets/rules/python/coverage/cpp_lib.cpp",
        None,
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_header_file_on_cpp_dep(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_file = "fbcode/buck2/tests/targets/rules/python/coverage/cpp_lib.h";
    let result = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/python/coverage:test_with_cpp_dep",
            folders: &[],
            files: &[header_file],
            mode: None,
        },
    )
    .await?;

    assert!(result.is_empty(), "{result:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_python_coverage_filtering_by_file_on_ligen_cpp_dep(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/python/coverage:test_with_ligen_cpp_dep",
        "fbcode/buck2/tests/targets/rules/python/coverage/adder_ligen.cpp",
        None,
    )
    .await
}

#[buck_test(inplace = true)]
async fn test_python_selective_coverage_in_arvr(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    assert_only_file_covered(
        buck,
        tmp_path,
        "fbsource//arvr/projects/tcc_playground/python_unittest:test_example",
        "arvr/projects/tcc_playground/python_unittest/lib/add.py",
        Some("@fbsource//arvr/mode/platform010/opt"),
    )
    .await
}
