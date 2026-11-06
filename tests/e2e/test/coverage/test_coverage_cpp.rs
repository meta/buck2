/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::Context;
use buck2_e2e::prelude::*;

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

/// The command lines of the C++ compilations `target` depends on, by target and identifier, with
/// `selected_files` selected for coverage if given.
async fn compile_commands(
    buck: &Buck,
    target: &str,
    selected_files: Option<&[&str]>,
) -> anyhow::Result<BTreeMap<String, String>> {
    let query = format!("kind(run, deps('{target}'))");
    let mut args = vec![
        query.as_str(),
        "--output-attribute=^(category|identifier|cmd)$",
        "--json",
    ];
    let file_path_filter =
        selected_files.map(|files| format!("code_coverage.file_path_filter={}", files.join(":")));
    if let Some(file_path_filter) = &file_path_filter {
        args.extend([
            "-c",
            "code_coverage.enable=filtered",
            "-c",
            file_path_filter,
        ]);
    }
    let result = buck.aquery(&args).await?;
    let actions: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&result.stdout)?;
    let mut commands = BTreeMap::new();
    for (key, action) in &actions {
        let attr = |name: &str| {
            action
                .get(name)
                .and_then(|v| v.as_str())
                .with_context(|| format!("No `{name}` of `{key}`"))
        };
        if attr("category")? == "cxx_compile" {
            let label = key
                .split('`')
                .nth(1)
                .with_context(|| format!("No label in `{key}`"))?
                .split(" (")
                .next()
                .unwrap_or_default();
            commands.insert(
                format!("{label} {}", attr("identifier")?),
                attr("cmd")?.to_owned(),
            );
        }
    }
    assert!(!commands.is_empty());
    Ok(commands)
}

#[buck_test(inplace = true)]
async fn test_cpp_coverage_private_dep_cache_reuse(buck: &Buck) -> anyhow::Result<()> {
    let package = "fbcode//testing_frameworks/code_coverage/playground";
    let source_dir = "fbcode/testing_frameworks/code_coverage/playground/";
    let header = format!("{source_dir}ThirdLevelDep.h");
    let test = format!("{package}:test");
    let ordinary = compile_commands(buck, &test, None).await?;
    let selected = compile_commands(buck, &test, Some(&[&header])).await?;
    let unchanged = [
        format!("{package}:first_level_dep "),
        format!("{package}:test "),
    ];
    let instrumented = [
        format!("{package}:third_level_dep "),
        format!("{package}:second_level_dep_that_includes_third_level_dep "),
    ];
    for prefix in unchanged.iter().chain(&instrumented) {
        assert!(
            selected.keys().any(|key| key.starts_with(prefix.as_str())),
            "{prefix}"
        );
    }
    for (key, command) in &selected {
        if unchanged.iter().any(|p| key.starts_with(p.as_str())) {
            let ordinary_command = ordinary
                .get(key)
                .with_context(|| format!("No `{key}` without coverage"))?;
            assert_eq!(command, ordinary_command, "{key}");
        } else if instrumented.iter().any(|p| key.starts_with(p.as_str())) {
            assert!(command.contains("-fprofile-instr-generate"), "{key}");
        }
    }
    Ok(())
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
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &["xplat/folly"],
            files: &[],
            mode: None,
        },
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
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &["fbcode/buck2/tests"],
            files: &[],
            mode: None,
        },
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
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &["fbcode/buck2/tests"],
            files: &[],
            mode: Some("@fbcode//mode/dev-lg"),
        },
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
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &["xplat/folly"],
            files: &[],
            mode: Some("@fbcode//mode/dev-lg"),
        },
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
    let source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &[],
            files: &[source_name],
            mode: Some("@fbcode//mode/dev-lg"),
        },
    )
    .await?;

    assert!(contains(&paths, source_name), "{paths:?}");
    let unexpected_paths: Vec<_> = paths.iter().filter(|p| *p != source_name).collect();
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
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &[],
            files: &[source_name],
            mode: Some("@fbcode//mode/dev-lg"),
        },
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
    let header_name = "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightRightOnlyUsedHere.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_link_groups",
            folders: &[],
            files: &[header_name],
            mode: Some("@fbcode//mode/dev-lg"),
        },
    )
    .await?;

    assert!(paths.is_empty(), "{paths:?}");
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_header_file_defined_in_one_link_group_and_used_in_another(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_header_used_in_different_link_group",
            folders: &[],
            files: &[header_name],
            mode: Some("@fbcode//mode/dev-lg"),
        },
    )
    .await?;

    assert_eq!(paths, [header_name]);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_header_file_defined_in_one_link_group_and_used_in_test_binary_link_group(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "fbcode/testing_frameworks/code_coverage/playground/link_groups/LibraryRightLeftUsedInOtherLinkGroup.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground/link_groups:test_with_header_used_in_test_binary",
            folders: &[],
            files: &[header_name],
            mode: Some("@fbcode//mode/dev-lg"),
        },
    )
    .await?;

    assert_eq!(paths, [header_name]);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &[],
            files: &[source_name],
            mode: None,
        },
    )
    .await?;

    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_eq!(paths[0], source_name);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_test_binary_header_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "fbcode/testing_frameworks/code_coverage/playground/Test.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground:test",
            folders: &[],
            files: &[header_name],
            mode: None,
        },
    )
    .await?;

    assert_eq!(paths, [header_name]);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_library_header_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let header_name = "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDep.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground:test",
            folders: &[],
            files: &[header_name],
            mode: None,
        },
    )
    .await?;

    assert_eq!(paths, [header_name]);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_when_filter_by_library_private_header_file(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let private_header_name =
        "fbcode/testing_frameworks/code_coverage/playground/ThirdLevelDepPrivate.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground:test",
            folders: &[],
            files: &[private_header_name],
            mode: None,
        },
    )
    .await?;

    assert_eq!(paths.len(), 3, "{paths:?}");
    assert!(contains(&paths, private_header_name));
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
    let header_name = "fbcode/testing_frameworks/code_coverage/playground/LibraryWithOnlyHeaders.h";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//testing_frameworks/code_coverage/playground:test_with_dep_with_only_headers",
            folders: &[],
            files: &[header_name],
            mode: None,
        },
    )
    .await?;

    assert_eq!(paths, [header_name]);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file_with_opt_mode(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &[],
            files: &[source_name],
            mode: Some("@fbcode//mode/opt"),
        },
    )
    .await?;

    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_eq!(paths[0], source_name);
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_cpp_test_coverage_filter_by_file_and_path(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let source_name = "fbcode/buck2/tests/targets/rules/cxx/cpp_test_pass.cpp";
    let paths = collect_coverage_for(
        buck,
        tmp_path,
        CoverageRun {
            target: "fbcode//buck2/tests/targets/rules/cxx:cpp_test_pass",
            folders: &["xplat/folly"],
            files: &[source_name],
            mode: None,
        },
    )
    .await?;

    assert!(
        paths.iter().any(|p| p.starts_with("xplat/folly")),
        "expected to find some folly sources in {paths:?}"
    );

    let source_paths = paths.iter().filter(|p| *p == source_name).count();
    assert_eq!(
        source_paths, 1,
        "expected to find {source_name} in {paths:?}"
    );

    let unexpected_paths: Vec<_> = paths
        .iter()
        .filter(|p| *p != source_name && !is_folly(p))
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
        CoverageRun {
            target: "fbsource//xplat/testinfra/playground/cpp:example_testFbcode",
            folders: &[],
            files: &[file_to_collect_coverage],
            mode: None,
        },
    )
    .await?;

    assert_eq!(
        paths,
        [file_to_collect_coverage],
        "Should collect coverage only for {file_to_collect_coverage} but got coverage for {paths:?}"
    );
    Ok(())
}
