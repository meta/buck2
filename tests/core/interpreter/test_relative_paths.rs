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

#[buck_test]
async fn test_relative_path_basic(buck: &Buck) -> anyhow::Result<()> {
    assert!(
        buck.targets(&["//foo/bar:"])
            .await?
            .stdout
            .contains("//foo/bar:test_basic")
    );
    Ok(())
}

#[buck_test]
async fn test_relative_path_left_allowed_dir(buck: &Buck) {
    buck.targets(&["//foo/baz:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Relative import path `../../defs.bzl` is not allowed at the current location.",
        );
}

#[buck_test]
async fn test_relative_path_has_symlink(buck: &Buck) -> anyhow::Result<()> {
    symlink_dir(buck.cwd(), &buck.cwd().join("foo/sym"))?;
    buck.targets(&["//foo/sym/foo/bar:"])
        .await
        .expect_failure()
        .assert_stderr_matches(
            "Symlink found on the way from current dir `root//foo/sym/foo/bar` to allowed relative dir `root//foo`: `root//foo/sym`.",
        );
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

#[buck_test]
async fn test_relative_path_in_attribute_default_current(buck: &Buck) {
    buck.targets(&["//foo/default_current:target"])
        .await
        .expect_failure()
        .assert_stderr_matches("Target pattern must be absolute");
}

#[buck_test]
async fn test_relative_path_in_attribute_default_up(buck: &Buck) {
    buck.targets(&["//foo/default_up:target"])
        .await
        .expect_failure()
        .assert_stderr_matches("Target pattern must be absolute");
}
