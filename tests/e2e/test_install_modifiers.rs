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

// Currently installer grpc doesn't compile on Mac
#[cfg(target_os = "linux")]
#[buck_test(inplace = true)]
async fn test_install_modifiers(buck: &Buck, tmp_path: &Path) -> anyhow::Result<()> {
    let tmp_dir = tmp_path.join("no_modifiers");
    std::fs::create_dir(&tmp_dir)?;
    let dst = format!("{}/", tmp_dir.display());

    buck.install(&[
        "fbcode//buck2/tests/targets/rules/install:installer_modifiers_test",
        "--",
        "--dst",
        &dst,
    ])
    .await?;

    assert!(tmp_dir.join("default").exists());

    let tmp_dir = tmp_path.join("modifiers");
    std::fs::create_dir(&tmp_dir)?;
    let dst = format!("{}/", tmp_dir.display());

    buck.install(&[
        "fbcode//buck2/tests/targets/rules/install:installer_modifiers_test?asan",
        "--",
        "--dst",
        &dst,
    ])
    .await?;

    assert!(tmp_dir.join("asan").exists());
    Ok(())
}

#[buck_test(inplace = true)]
async fn test_install_fails_with_global_modifiers(
    buck: &Buck,
    tmp_path: &Path,
) -> anyhow::Result<()> {
    let tmp_dir = tmp_path.join("install_test");
    std::fs::create_dir(&tmp_dir)?;
    let dst = format!("{}/", tmp_dir.display());
    buck.install(&[
        "--modifier",
        "asan",
        "fbcode//buck2/tests/targets/rules/install:installer_modifiers_test?asan",
        "--",
        "--dst",
        &dst,
    ])
    .await
    .expect_failure()
    .assert_stderr_matches(r"Cannot specify modifiers with \?modifier syntax when global CLI modifiers are set with --modifier flag");
    Ok(())
}
