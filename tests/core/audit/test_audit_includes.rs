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

use buck2_e2e::BuckOutput;
use buck2_e2e::prelude::*;

fn includes(output: &BuckOutput) -> Vec<String> {
    let mut includes: Vec<String> = output
        .stdout
        .lines()
        .filter(|line| line.ends_with(".bzl") || line.ends_with(".json"))
        .map(|line| line.rsplit(['/', '\\']).next().unwrap_or(line).to_owned())
        .collect();
    includes.sort();
    includes
}

#[buck_test]
async fn test_audit_includes(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let expected_includes = ["example.json", "incl.bzl", "prelude.bzl"];
    // Using project relative path.
    let output = buck.audit(&["includes", "TARGETS.fixture"]).await?;
    assert_eq!(includes(&output), expected_includes);

    // Using project relative path when in a subdirectory.
    let output = buck
        .audit(&["includes", "TARGETS.fixture"])
        .rel_cwd("dir")
        .await?;
    assert_eq!(includes(&output), expected_includes);

    // Using absolute path.
    let output = buck
        .audit(&[
            "includes",
            &format!("{}/TARGETS.fixture", buck.cwd().display()),
        ])
        .await?;
    assert_eq!(includes(&output), expected_includes);

    if !cfg!(windows) {
        // Create symlink to the project root in a temporary directory.
        symlink_dir(buck.cwd(), &tmp_path.join("symlink"))?;

        let output = buck
            .audit(&[
                "includes",
                &format!("{}/symlink/TARGETS.fixture", tmp_path.display()),
            ])
            .await?;
        assert_eq!(includes(&output), expected_includes);
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}
