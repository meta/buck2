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

/// The `srcs` of `target`, as paths from the repository root.
async fn query_target_srcs(buck: &Buck, target: &str) -> anyhow::Result<Vec<String>> {
    let query_result = buck.uquery(&[target, "--output-attribute", "srcs"]).await?;
    let json: serde_json::Value = serde_json::from_str(&query_result.stdout)?;
    json.get(target)
        .and_then(|attrs| attrs.get("srcs"))
        .and_then(|srcs| srcs.as_array())
        .with_context(|| format!("No `srcs` of `{target}` in:\n{}", query_result.stdout))?
        .iter()
        .map(|p| {
            Ok(p.as_str()
                .context("A non-string source")?
                .replace("fbcode//", "fbcode/"))
        })
        .collect()
}

/// Leaves out cxx.h, the CXX bridge framework header.
fn without_cxx_h(paths: Vec<String>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|p| !p.ends_with("/cxx.h"))
        .collect()
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//buck2/tests/targets/rules/rust:tests_pass",
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", coverage_file.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&coverage_file)?;
    assert!(
        paths
            .iter()
            .any(|p| p == "fbcode/buck2/tests/targets/rules/rust/tests_pass.rs"),
        "{paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_filtering_by_path_of_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/rust:tests_pass",
        &["fbcode/buck2/tests"],
        &[],
    )
    .await?;

    let unexpected_paths: Vec<_> = paths
        .iter()
        .filter(|p| !p.starts_with("fbcode/buck2/tests"))
        .collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_filtering_by_path_of_files(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let target = "fbcode//buck2/tests/targets/rules/rust:tests_pass";
    let file_path = "fbcode/buck2/tests/targets/rules/rust/tests_pass.rs";
    let paths = collect_coverage_for(buck, tmp_path, target, &[], &[file_path]).await?;

    let mut target_srcs = query_target_srcs(buck, target).await?;
    target_srcs.sort();

    // rust can only do coverage at crate granularity,
    // so we expect coverage for all the files in the target
    assert_eq!(paths, target_srcs, "{target_srcs:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_rust_library_filtering_by_file_path_outside_of_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_path = "fbcode/testing_frameworks/code_coverage/adder.rs";
    let lib_target = "fbcode//testing_frameworks/code_coverage:adder";

    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "buck2/tests/targets/rules/rust/coverage/test_with_rust_library_outside_targets_path:test",
        &[],
        &[file_path],
    )
    .await?;

    let mut target_srcs = query_target_srcs(buck, lib_target).await?;
    target_srcs.sort();
    assert_eq!(paths, target_srcs, "{target_srcs:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_rust_library_filtering_by_folder_path_outside_of_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "buck2/tests/targets/rules/rust/coverage/test_with_rust_library_outside_targets_path:test",
        &["fbcode/testing_frameworks"],
        &[],
    )
    .await?;

    let fbcode_filename = "fbcode/testing_frameworks/code_coverage/adder.rs";
    assert!(paths.iter().any(|p| p == fbcode_filename), "{paths:?}");
    let unexpected_paths: Vec<_> = paths.iter().filter(|p| *p != fbcode_filename).collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_cxx(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "fbcode/testing_frameworks/code_coverage/rust/Adder.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_cxx_cpp_dep",
        &[],
        &[file_to_collect_coverage],
    )
    .await?;

    // cxx.h is a CXX bridge framework header whose buck-out path now resolves
    // to a source path due to -fcoverage-prefix-map; filter it out.
    let paths = without_cxx_h(paths);
    assert_eq!(paths, [file_to_collect_coverage], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_cxx_through_rust_library(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "fbcode/testing_frameworks/code_coverage/rust/Adder.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_cxx_rust_library_dep",
        &[],
        &[file_to_collect_coverage],
    )
    .await?;

    let paths = without_cxx_h(paths);
    assert_eq!(paths, [file_to_collect_coverage], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_cxx_on_autogenerated_library_unittests(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "fbcode/testing_frameworks/code_coverage/rust/Adder.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_in_library-unittest",
        &[],
        &[file_to_collect_coverage],
    )
    .await?;

    let paths = without_cxx_h(paths);
    assert_eq!(paths, [file_to_collect_coverage], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_bindgen_rust_library(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "fbcode/testing_frameworks/code_coverage/rust/AdderC.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_bindgen_rust_library_dep",
        &[],
        &[file_to_collect_coverage],
    )
    .await?;

    assert_eq!(paths, [file_to_collect_coverage], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_ligen_cpp_dep(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "fbcode/testing_frameworks/code_coverage/rust/AdderLigen.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:test_with_ligen_cpp_dep",
        &[],
        &[file_to_collect_coverage],
    )
    .await?;

    assert_eq!(paths, [file_to_collect_coverage], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_header_with_cxx(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "fbcode/testing_frameworks/code_coverage/rust/AdderWithHeaderCode.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_code_in_cpp_header",
        &[],
        &[header_name],
    )
    .await?;

    // cxx.h is a CXX bridge framework header whose buck-out path now resolves
    // to a source path due to -fcoverage-prefix-map; filter it out.
    let paths = without_cxx_h(paths);
    assert_eq!(paths, [header_name]);
    Ok(())
}
