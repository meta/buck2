/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use buck2_e2e::prelude::*;

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_bxl_fs_exists(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[
        "//bxl:fs.bxl:exists",
        "--",
        "--root_path",
        &buck.cwd().to_string_lossy(),
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_bxl_fs_exists_symlink(buck: &Buck) -> anyhow::Result<()> {
    let link_path = buck.cwd().join("symlink/foo/bar");
    if !std::fs::symlink_metadata(&link_path)?.is_symlink() {
        std::fs::remove_file(&link_path)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink("../bar", &link_path)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file("../bar", &link_path)?;
    }
    buck.bxl(&["//bxl:fs.bxl:exists_symlink"]).await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_bxl_fs_list(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:fs.bxl:list_relative_path"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["root//bin/TARGETS.fixture", "root//bin/kind"]
    );

    let result = buck
        .bxl(&[
            "//bxl:fs.bxl:list_absolute_path",
            "--",
            "--root_path",
            &buck.cwd().to_string_lossy(),
        ])
        .await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["root//bin/TARGETS.fixture", "root//bin/kind"]
    );

    let result = buck.bxl(&["//bxl:fs.bxl:list_source_artifact"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["root//bin/kind/TARGETS.fixture", "root//bin/kind/rules.bzl"]
    );

    let result = buck.bxl(&["//bxl:fs.bxl:list_file_node"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["root//bin/kind/TARGETS.fixture", "root//bin/kind/rules.bzl"]
    );

    let result = buck.bxl(&["//bxl:fs.bxl:list_dirs_only"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["root//bin/kind"]
    );

    let result = buck.bxl(&["//bxl:fs.bxl:list_cell_path"]).await?;

    let expected_output = serde_json::json!(["root//bin/TARGETS.fixture", "root//bin/kind"]);

    let output: serde_json::Value = serde_json::from_str(&result.stdout)?;
    assert_eq!(output["@root//bin"], expected_output);
    assert_eq!(output["root//bin"], expected_output);
    assert_eq!(output["//bin"], expected_output);
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_bxl_fs_is_file(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[
        "//bxl:fs.bxl:is_file",
        "--",
        "--root_path",
        &buck.cwd().to_string_lossy(),
    ])
    .await?;
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", allow_soft_errors = true)]
async fn test_bxl_fs_is_dir(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&[
        "//bxl:fs.bxl:is_dir",
        "--",
        "--root_path",
        &buck.cwd().to_string_lossy(),
    ])
    .await?;
    Ok(())
}

#[buck_test(
    inplace = false,
    data_dir = "bxl/simple",
    skip_for_os = ["windows"],
    allow_soft_errors = true
)]
async fn test_bxl_fs_project_rel_path(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:fs.bxl:project_rel_path"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        ["bin/kind/TARGETS.fixture", "bin/kind/rules.bzl"]
    );
    Ok(())
}

#[buck_test(
    inplace = false,
    data_dir = "bxl/simple",
    skip_for_os = ["windows"],
    allow_soft_errors = true
)]
async fn test_bxl_fs_abs_path_unsafe(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.bxl(&["//bxl:fs.bxl:abs_path_unsafe"]).await?;

    assert_eq!(
        result.stdout.lines().collect::<Vec<_>>(),
        [
            buck.cwd()
                .join("bin/kind/TARGETS.fixture")
                .to_string_lossy(),
            buck.cwd().join("bin/kind/rules.bzl").to_string_lossy(),
        ]
    );
    Ok(())
}

#[buck_test(inplace = false, data_dir = "bxl/simple", skip_for_os = ["windows"])]
async fn test_bxl_fs_source(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl:fs.bxl:source"]).await?;

    buck.bxl(&["//bxl:fs.bxl:source_invalid_path"])
        .await
        .expect_failure()
        .assert_stderr_matches("Inferred package path `root//fs` is not a valid package within the given file path `root//this/path/does/not/exist");
    buck.bxl(&["//bxl:fs.bxl:source_invalid_hint"])
        .await
        .expect_failure()
        .assert_stderr_matches("Inferred package path `root//bin/kind` is not a valid package within the given file path `root//fs/src/source");
    buck.bxl(&["//bxl:fs.bxl:source_too_many_hints"])
        .await
        .expect_failure()
        .assert_stderr_matches("Expected a single target hint, not an iterable");
    Ok(())
}

#[buck_test(
    inplace = false,
    data_dir = "bxl/simple",
    skip_for_os = ["windows"],
    allow_soft_errors = true
)]
async fn test_bxl_file_set_ops(buck: &Buck) -> anyhow::Result<()> {
    buck.bxl(&["//bxl/fs.bxl:file_set_operations"]).await?;
    Ok(())
}
