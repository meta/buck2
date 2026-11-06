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

// TODO(nga): Local and remote execution of `//:dog_and_bone` must produce identical output.
//   This is a known limitation of at least our RE implementation. It reads through symlinks.

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_preserves_empty_directory_local(buck: &Buck) -> anyhow::Result<()> {
    let result = buck
        .build(&["//:dog_and_bone", "--prefer-local", "--no-remote-cache"])
        .await?;
    let out = result
        .build_report()?
        .output_for_target("//:dog_and_bone")?;
    assert!(out.is_symlink());
    assert!(out.is_file());
    Ok(())
}

#[buck_test(skip_for_os = ["windows"])]
async fn test_symlink_preserves_empty_directory_remote(buck: &Buck) -> anyhow::Result<()> {
    let result = buck.build(&["//:dog_and_bone", "--prefer-remote"]).await?;
    let out = result
        .build_report()?
        .output_for_target("//:dog_and_bone")?;
    // This is incorrect, should be a symlink.
    assert!(!out.is_symlink());
    assert!(out.is_file());
    Ok(())
}

#[buck_test]
async fn test_noop(_buck: &Buck) {}
