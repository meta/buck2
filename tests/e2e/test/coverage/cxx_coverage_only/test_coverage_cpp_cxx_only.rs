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

/// The files the coverage of `target`'s tests covers, without duplicates, when only the C++ files
/// `filter` selects are instrumented. `mode` goes to `buck2 test` too.
async fn collect_coverage_for(
    buck: &Buck,
    tmp_path: &Path,
    target: &str,
    filter: &[&str],
    mode: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let coverage_file = tmp_path.join("coverage.txt");
    let cxx_coverage_only = format!("fbcode.cxx_coverage_only={}", filter.join(" "));
    let coverage_output = format!("--coverage-output={}", coverage_file.display());
    let mut args: Vec<&str> = mode.into_iter().collect();
    args.extend([
        "--config",
        "fbcode.coverage_selective=true",
        "--config",
        &cxx_coverage_only,
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

fn contains(paths: &[String], path: &str) -> bool {
    paths.iter().any(|p| p == path)
}

/// Whether `path` is under folly, or one of the third-party headers folly includes.
fn is_folly(path: &str) -> bool {
    path.starts_with("xplat/folly")
        || path.starts_with("fbcode/fbsource/folly/")
        || path.starts_with("third-party/fast_float/")
        || path.starts_with("third-party/libdwarf/")
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let coverage_file = tmp_path.join("coverage.txt");
    buck.test(&[
        "@fbcode//mode/dbgo-cov",
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        "--",
        "--collect-coverage",
        &format!("--coverage-output={}", coverage_file.display()),
    ])
    .await?;
    let paths = read_coverage_paths(&coverage_file)?;

    assert!(
        contains(
            &paths,
            "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp"
        ),
        "{paths:?}"
    );
    assert!(
        contains(&paths, "fbcode/common/gtest/LightMain.cpp"),
        "{paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_path_outside_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &["xplat/folly"],
        None,
    )
    .await?;

    assert!(
        paths.iter().any(|p| p.starts_with("xplat/folly")),
        "{paths:?}"
    );
    // third-party headers transitively included by folly now appear with real
    // source paths due to -fcoverage-prefix-map; allow them.
    let unexpected_paths: Vec<_> = paths.iter().filter(|p| !is_folly(p)).collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_path_of_target(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &["buck2/tests"],
        None,
    )
    .await?;

    assert!(
        paths.iter().any(|p| p.starts_with("fbcode/buck2/tests")),
        "{paths:?}"
    );
    let unexpected_paths: Vec<_> = paths
        .iter()
        .filter(|p| !p.starts_with("fbcode/buck2/tests"))
        .collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_path_of_target_with_dev_lg(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &["buck2/tests"],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    assert!(
        paths.iter().any(|p| p.starts_with("fbcode/buck2/tests")),
        "{paths:?}"
    );
    let unexpected_paths: Vec<_> = paths
        .iter()
        .filter(|p| !p.starts_with("fbcode/buck2/tests"))
        .collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_path_in_link_group_with_dev_lg(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &["xplat/folly"],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    assert!(
        paths.iter().any(|p| p.starts_with("xplat/folly")),
        "{paths:?}"
    );
    let unexpected_paths: Vec<_> = paths.iter().filter(|p| !is_folly(p)).collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file_of_target_with_dev_lg(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let source_name = "buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &[source_name],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    let fbcode_source_name = format!("fbcode/{source_name}");
    assert!(contains(&paths, &fbcode_source_name), "{paths:?}");
    let unexpected_paths: Vec<_> = paths.iter().filter(|p| **p != fbcode_source_name).collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_source_file_in_link_group_with_dev_lg(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let source_name = "xplat/folly/String.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &[source_name],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    assert!(contains(&paths, source_name), "{paths:?}");
    let unexpected_paths: Vec<_> = paths
        .iter()
        .filter(|p| *p != source_name && !p.ends_with(".h"))
        .collect();
    assert!(unexpected_paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_header_file_in_link_group_with_dev_lg(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name =
        "testing_frameworks/code_coverage/playground/link_groups/LibraryRightRightOnlyUsedHere.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_link_groups",
        &[header_name],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    assert_eq!(paths.len(), 5, "{paths:?}");
    // because it belongs to a target that has a header file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightRight.cpp"
    ));
    // because it belongs to a target that has a dependency that contains a header
    // file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRight.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/TestWithLinkGroups.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/TestWithLinkGroups.h"
    ));
    // because it has executable code used by LibraryRight.cpp
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h"
    ));
    // because they are not in the transitive rdeps of the cxx_library that has the header
    // that was selected for coverage
    assert!(!contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeft.cpp"
    ));
    assert!(!contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryLeft.cpp"
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_header_file_defined_in_one_link_group_and_used_in_another(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_header_used_in_different_link_group",
        &[header_name],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    assert_eq!(paths.len(), 5, "{paths:?}");
    // because it belongs to a target that has a header file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeft.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h"
    ));
    // because it belongs to a target that has a dependency that contains a header
    // file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRight.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/TestWithLinkGroups.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/TestWithLinkGroups.h"
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_header_file_defined_in_one_link_group_and_used_in_test_binary_link_group(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_header_used_in_test_binary",
        &[header_name],
        Some("@fbcode//mode/dev-lg"),
    )
    .await?;

    assert_eq!(paths.len(), 5, "{paths:?}");
    // because it belongs to a target that has a header file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeft.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h"
    ));
    // because it belongs to a target that has a dependency that contains a header
    // file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRight.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/TestWithLinkGroups.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/link_groups/TestWithLinkGroups.h"
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let source_name = "buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &[source_name],
        None,
    )
    .await?;

    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_eq!(paths[0], format!("fbcode/{source_name}"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_test_binary_header_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "testing_frameworks/code_coverage/playground/Test.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground:test",
        &[header_name],
        None,
    )
    .await?;

    assert_eq!(paths.len(), 2, "{paths:?}");
    let header_name = format!("fbcode/{header_name}");
    assert!(contains(&paths, &header_name));
    let source_name = "fbcode/testing_frameworks/code_coverage/playground/Test.cpp";
    assert!(contains(&paths, source_name));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_library_header_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "testing_frameworks/code_coverage/playground/ThirdLevelDep.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground:test",
        &[header_name],
        None,
    )
    .await?;

    assert_eq!(paths.len(), 9, "{paths:?}");
    // because it belongs to a target that has a header file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDep.cpp"
    ));
    // because it's header file selected for coverage and it has executable code
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDep.h"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDepPrivate.h"
    ));
    // because it belongs to a target that has a dependency that contains a header
    // file selected for coverage
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/SecondLevelDepIncludesThirdLevelDep.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/SecondLevelDepDoesNotIncludeThirdLevelDep.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/FirstLevelDep.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/Test.cpp"
    ));
    // because a cpp file (FirstLvelDep.cpp) that included it called a template function
    // defined inside it
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/FirstLevelDep.h"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/Test.h"
    ));

    // because they are not in the transitive rdeps of the cxx_library that has the header
    // that was selected for coverage
    assert!(!contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/SecondLevelDepWithoutDeps.cpp"
    ));
    assert!(!contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/SecondLevelDepWithoutDeps.h"
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_library_private_header_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let private_header_name = "testing_frameworks/code_coverage/playground/ThirdLevelDepPrivate.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground:test",
        &[private_header_name],
        None,
    )
    .await?;

    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(contains(&paths, &format!("fbcode/{private_header_name}")));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDep.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDepPrivate.h"
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_header_file_in_headers_only_library(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "testing_frameworks/code_coverage/playground/LibraryWithOnlyHeaders.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//testing_frameworks/code_coverage/playground:test_with_dep_with_only_headers",
        &[header_name],
        None,
    )
    .await?;

    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(contains(&paths, &format!("fbcode/{header_name}")));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/TestWithDepWithOnlyHeaders.cpp"
    ));
    assert!(contains(
        &paths,
        "fbcode/testing_frameworks/code_coverage/playground/TestWithDepWithOnlyHeaders.h"
    ));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file_with_opt_mode(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let source_name = "buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &[source_name],
        Some("@fbcode//mode/opt"),
    )
    .await?;

    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_eq!(paths[0], format!("fbcode/{source_name}"));
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file_and_path(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let source_name = "buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
        &[source_name, "xplat/folly"],
        None,
    )
    .await?;

    assert!(
        paths.iter().any(|p| p.starts_with("xplat/folly")),
        "expected to find some folly sources in {paths:?}"
    );

    let source_path = format!("fbcode/{source_name}");
    let source_paths = paths.iter().filter(|p| **p == source_path).count();
    assert_eq!(
        source_paths, 1,
        "expected to find {source_path} in {paths:?}"
    );

    let unexpected_paths: Vec<_> = paths
        .iter()
        .filter(|p| **p != source_path && !is_folly(p))
        .collect();
    assert!(
        unexpected_paths.is_empty(),
        "Only coverage for the test source file and files under xplat/folly should have been collected, but got {unexpected_paths:?}"
    );
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_xplat_filter_by_file_path(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let file_to_collect_coverage = "xplat/testinfra/playground/cpp/ExampleTest.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        "fbsource//xplat/testinfra/playground/cpp:example_testFbcode",
        &[file_to_collect_coverage],
        None,
    )
    .await?;

    assert_eq!(
        paths,
        [file_to_collect_coverage],
        "Should collect coverage only for {file_to_collect_coverage} but got coverage for {paths:?}"
    );
    Ok(())
}
