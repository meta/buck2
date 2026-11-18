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
async fn test_target_hashing_accepts_backreferencing_relative_paths(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    buck.targets(&[
        ":bin",
        "--show-target-hash",
        "--target-hash-file-mode=paths_only",
        "--target-hash-modified-paths=../.buckconfig",
    ])
    .rel_cwd("bin")
    .await?;

    // Paths outside of the project still fail
    buck.targets(&[
        "bin:bin",
        "--show-target-hash",
        "--target-hash-file-mode=paths_only",
        "--target-hash-modified-paths=../.buckconfig",
    ])
    .await
    .expect_failure()
    .assert_stderr_matches("relativize path.*against project root");

    if !cfg!(windows) {
        // Absolute path non-normalized paths should work
        symlink(buck.cwd(), &tmp_path.join("symlink"))?;

        buck.targets(&[
            ":bin",
            "--show-target-hash",
            "--target-hash-file-mode=paths_only",
            &format!(
                "--target-hash-modified-paths={}/symlink/.buckconfig",
                tmp_path.display()
            ),
        ])
        .rel_cwd("bin")
        .await?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(original: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

#[cfg(not(unix))]
fn symlink(_original: &Path, _link: &Path) -> std::io::Result<()> {
    unreachable!("the symlink is only made off Windows")
}
