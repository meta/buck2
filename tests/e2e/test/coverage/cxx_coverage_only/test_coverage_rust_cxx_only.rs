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
use regex::Regex;

/// The files the coverage of `target`'s tests covers, without duplicates, when only the C++ files
/// `filter` selects are instrumented.
async fn collect_coverage_for(
    buck: &Buck,
    tmp_path: &Path,
    target: &str,
    filter: &[&str],
) -> anyhow::Result<Vec<String>> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "--config",
        "fbcode.coverage_selective=true",
        "--config",
        &format!("fbcode.cxx_coverage_only={}", filter.join(" ")),
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

/// Whether `regex` matches the whole of one of `items`.
fn any_item_matches(items: &[String], regex: &str) -> bool {
    let regex = Regex::new(&format!("^(?:{regex})$")).expect("a valid regex");
    items.iter().any(|i| regex.is_match(i))
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
        &["buck2/tests"],
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
async fn test_rust_test_coverage_of_rust_library_filtering_by_path_outside_of_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "buck2/tests/targets/rules/rust/coverage/test_with_rust_library_outside_targets_path:test",
        &["testing_frameworks"],
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
    let file_to_collect_coverage = "testing_frameworks/code_coverage/rust/Adder.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_cxx_cpp_dep",
        &[file_to_collect_coverage],
    )
    .await?;

    let fbcode_filename = format!("fbcode/{file_to_collect_coverage}");
    // cxx.h is a CXX bridge framework header whose buck-out path now resolves
    // to a source path due to -fcoverage-prefix-map; filter it out.
    let paths = without_cxx_h(paths);
    assert_eq!(paths, [fbcode_filename], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_cxx_through_rust_library(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "testing_frameworks/code_coverage/rust/Adder.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_cxx_rust_library_dep",
        &[file_to_collect_coverage],
    )
    .await?;

    let fbcode_filename = format!("fbcode/{file_to_collect_coverage}");
    let paths = without_cxx_h(paths);
    assert_eq!(paths, [fbcode_filename], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_cxx_on_autogenerated_library_unittests(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "testing_frameworks/code_coverage/rust/Adder.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_in_library-unittest",
        &[file_to_collect_coverage],
    )
    .await?;

    let fbcode_filename = format!("fbcode/{file_to_collect_coverage}");
    let paths = without_cxx_h(paths);
    assert_eq!(paths, [fbcode_filename], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_bindgen_rust_library(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "testing_frameworks/code_coverage/rust/AdderC.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_bindgen_rust_library_dep",
        &[file_to_collect_coverage],
    )
    .await?;

    let fbcode_filename = format!("fbcode/{file_to_collect_coverage}");
    assert_eq!(paths, [fbcode_filename], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_file_with_ligen_cpp_dep(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "testing_frameworks/code_coverage/rust/AdderLigen.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:test_with_ligen_cpp_dep",
        &[file_to_collect_coverage],
    )
    .await?;

    let fbcode_filename = format!("fbcode/{file_to_collect_coverage}");
    assert_eq!(paths, [fbcode_filename], "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_rust_test_coverage_of_cpp_file_filtering_by_header_with_cxx(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "testing_frameworks/code_coverage/rust:tests_with_code_in_cpp_header",
        &["testing_frameworks/code_coverage/rust/AdderWithHeaderCode.h"],
    )
    .await?;

    // cxx.h is a CXX bridge framework header whose buck-out path now resolves
    // to a source path due to -fcoverage-prefix-map; filter it out.
    let paths = without_cxx_h(paths);
    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(
        paths
            .iter()
            .any(|p| p == "fbcode/testing_frameworks/code_coverage/rust/AdderWithHeaderCode.cpp"),
        "{paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|p| p == "fbcode/testing_frameworks/code_coverage/rust/AdderWithHeaderCode.h"),
        "{paths:?}"
    );
    assert!(
        any_item_matches(
            &paths,
            r"fbcode/testing_frameworks/code_coverage/rust/__tests_with_code_in_cpp_header-bridge_generated.cc__/[a-z0-9]+/out/generated.cc",
        ),
        "{paths:?}"
    );
    Ok(())
}
