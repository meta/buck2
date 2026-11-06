/*
 * Copyright (c) Meta Platforms, Inc. and affiliates.
 *
 * This source code is dual-licensed under either the MIT license found in the
 * LICENSE-MIT file in the root directory of this source tree or the Apache
 * License, Version 2.0 found in the LICENSE-APACHE file in the root directory
 * of this source tree. You may select, at your option, one of the
 * above-listed licenses.
 */

use std::path::Path;

use buck2_e2e::prelude::*;

/// The directory the test's project is in.
fn project_parent(buck: &Buck) -> &Path {
    buck.cwd()
        .parent()
        .expect("the project is in a temporary directory")
}

#[buck_test(data_dir = "include_external")]
async fn test_include_external_file(buck: &Buck) -> anyhow::Result<()> {
    // Note that the repo is inside a tempdir
    std::fs::write(project_parent(buck).join("extra"), "[abc]\ndef=x")?;
    buck.audit_config(&["--cell", "root"])
        .await
        .expect_failure()
        .assert_stderr_matches("Improperly include directive path");
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[buck_test(data_dir = "empty", skip_for_os = ["windows"])]
async fn test_external_symlink_resolution(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let base = tmp_path.join("base");
    std::fs::create_dir_all(base.join("b").join("bb"))?;
    std::fs::create_dir(base.join("a"))?;
    symlink(Path::new("../b/bb"), &base.join("a").join("aa"))?;
    std::fs::write(base.join("b").join("included"), "[sec]\nval = physical")?;
    std::fs::write(base.join("a").join("included"), "[sec]\nval = logical")?;

    std::fs::write(
        base.join("b").join("bb").join("config"),
        "<file:../included>",
    )?;

    let config_via_symlink = base.join("a").join("aa").join("config");

    let res = buck
        .audit_config(&[
            "--cell",
            "root",
            "--config-file",
            &config_via_symlink.to_string_lossy(),
        ])
        .await?;
    assert!(res.stdout.contains("val = physical"));
    Ok(())
}

#[buck_test(data_dir = "empty")]
async fn test_changing_external_include(buck: &Buck) -> anyhow::Result<()> {
    let extra = project_parent(buck).join("extra");
    std::fs::write(&extra, "[abc]\n  def = 1")?;

    // Start the daemon and build once
    buck.audit_config(&["--all-cells"])
        .env("BUCK2_TEST_EXTRA_EXTERNAL_CONFIG", &extra)
        .await?;

    // Change the file and build again
    std::fs::write(&extra, "[abc]\n    def = 2")?;

    let res = buck.audit_config(&["--cell", "root", "abc.def"]).await?;
    assert!(res.stdout.contains("[abc]\n    def = 2"));
    let res = buck.audit_config(&["--cell", "cell", "abc.def"]).await?;
    assert!(res.stdout.contains("[abc]\n    def = 2"));
    Ok(())
}

#[buck_test(data_dir = "include_through_symlink", allow_soft_errors = true)]
async fn test_external_symlink_source_file(buck: &Buck) -> anyhow::Result<()> {
    let external_dir = project_parent(buck).join("extra");
    std::fs::create_dir(&external_dir)?;
    symlink(&external_dir, &buck.cwd().join("repo_dir"))?;

    buck.audit_config(&["--cell", "root", "abc.def"]).await?;
    Ok(())
}
